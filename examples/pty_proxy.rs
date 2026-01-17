//! PTY proxy example using nix
//!
//! Usage: cargo run --example pty_proxy -- <program> [args...]

use nix::libc;
use nix::pty::{openpty, OpenptyResult};
use nix::sys::select::{select, FdSet};
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{execvp, fork, isatty, read, setsid, write, ForkResult, Pid};
use std::ffi::CString;
use std::io::Write as _;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};

static SIGWINCH_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigwinch(_: libc::c_int) {
    SIGWINCH_RECEIVED.store(true, Ordering::SeqCst);
}

fn set_nonblocking(fd: libc::c_int) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

fn get_window_size() -> Option<libc::winsize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if ret == 0 {
        Some(ws)
    } else {
        None
    }
}

fn set_window_size(fd: libc::c_int, ws: &libc::winsize) {
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, ws) };
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: pty_proxy <program> [args...]");
        std::process::exit(1);
    }

    // Convert args to CStrings for execvp
    let program = CString::new(args[0].as_str())?;
    let c_args: Vec<CString> = args
        .iter()
        .map(|s| CString::new(s.as_str()).unwrap())
        .collect();

    // Check if stdin is a terminal and save original terminal settings
    let stdin = std::io::stdin();
    let is_tty = isatty(&stdin).unwrap_or(false);
    let original_termios = if is_tty {
        Some(tcgetattr(&stdin)?)
    } else {
        None
    };

    // Open PTY pair
    let OpenptyResult { master, slave } = openpty(None, None)?;
    let master_raw = master.as_raw_fd();
    let slave_raw = slave.as_raw_fd();

    // Set initial window size
    if let Some(ws) = get_window_size() {
        set_window_size(master_raw, &ws);
    }

    // Fork
    match unsafe { fork() }? {
        ForkResult::Child => {
            // Close master in child
            drop(master);

            // Create new session and set controlling terminal
            setsid()?;

            // Set controlling terminal
            unsafe { libc::ioctl(slave_raw, libc::TIOCSCTTY as libc::c_ulong, 0) };

            // Redirect stdio to slave PTY using libc dup2
            unsafe {
                libc::dup2(slave_raw, libc::STDIN_FILENO);
                libc::dup2(slave_raw, libc::STDOUT_FILENO);
                libc::dup2(slave_raw, libc::STDERR_FILENO);
            }

            // Close original slave fd if it's not one of the standard fds
            if slave_raw > 2 {
                drop(slave);
            }

            // Execute the program
            execvp(&program, &c_args)?;
            unreachable!()
        }
        ForkResult::Parent { child } => {
            // Close slave in parent
            drop(slave);

            // Set up SIGWINCH handler
            let sa = SigAction::new(
                SigHandler::Handler(handle_sigwinch),
                SaFlags::SA_RESTART,
                SigSet::empty(),
            );
            unsafe { sigaction(Signal::SIGWINCH, &sa)? };

            // Set terminal to raw mode if we have a terminal
            if let Some(ref orig) = original_termios {
                let mut raw_termios = orig.clone();
                cfmakeraw(&mut raw_termios);
                tcsetattr(&stdin, SetArg::TCSANOW, &raw_termios)?;
            }

            // Set master to non-blocking
            set_nonblocking(master_raw);

            // Main I/O loop
            let result = run_io_loop(&master, child);

            // Restore terminal settings
            if let Some(ref orig) = original_termios {
                let _ = tcsetattr(&stdin, SetArg::TCSANOW, orig);
            }

            result?;
        }
    }

    Ok(())
}

fn run_io_loop(master: &OwnedFd, child: Pid) -> Result<(), Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let master_raw = master.as_raw_fd();
    let mut buf = [0u8; 4096];
    let mut exit_code: Option<i32> = None;

    loop {
        // Check for window size changes
        if SIGWINCH_RECEIVED.swap(false, Ordering::SeqCst) {
            if let Some(ws) = get_window_size() {
                set_window_size(master_raw, &ws);
            }
        }

        // Check if child has exited (only if we haven't already detected it)
        if exit_code.is_none() {
            match waitpid(child, Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::Exited(_, code)) => {
                    exit_code = Some(code);
                }
                Ok(WaitStatus::Signaled(_, sig, _)) => {
                    exit_code = Some(128 + sig as i32);
                }
                Ok(WaitStatus::StillAlive) => {}
                Ok(_) => {}
                Err(nix::errno::Errno::ECHILD) => {
                    exit_code = Some(0);
                }
                Err(e) => return Err(e.into()),
            }
        }

        // Set up select
        let mut read_fds = FdSet::new();
        if exit_code.is_none() {
            read_fds.insert(stdin.as_fd());
        }
        read_fds.insert(master.as_fd());

        let mut timeout = nix::sys::time::TimeVal::new(0, 100_000); // 100ms timeout
        match select(None, Some(&mut read_fds), None, None, Some(&mut timeout)) {
            Ok(0) => {
                // Timeout - if child has exited and we've had a timeout, we're done
                if exit_code.is_some() {
                    break;
                }
                continue;
            }
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(e.into()),
        }

        // Read from stdin, write to master (only if child is still running)
        if exit_code.is_none() && read_fds.contains(stdin.as_fd()) {
            match read(stdin.as_fd(), &mut buf) {
                Ok(0) => {
                    // EOF on stdin, but keep reading from master
                }
                Ok(n) => {
                    let _ = write(master, &buf[..n]);
                }
                Err(nix::errno::Errno::EAGAIN) => {}
                Err(e) => return Err(e.into()),
            }
        }

        // Read from master, write to stdout
        if read_fds.contains(master.as_fd()) {
            match read(master.as_fd(), &mut buf) {
                Ok(0) => {
                    // EOF on master
                    break;
                }
                Ok(n) => {
                    let mut stdout_lock = stdout.lock();
                    let _ = stdout_lock.write_all(&buf[..n]);
                    let _ = stdout_lock.flush();
                }
                Err(nix::errno::Errno::EAGAIN) => {
                    // No data available right now, continue
                }
                Err(nix::errno::Errno::EIO) => {
                    // PTY closed - this is expected after child exits
                    break;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    if let Some(code) = exit_code {
        std::process::exit(code);
    }

    Ok(())
}
