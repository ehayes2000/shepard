use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use nix::libc;
use nix::pty::{OpenptyResult, openpty};
use nix::sys::select::{FdSet, select};
use nix::unistd::{ForkResult, execvp, fork, read, setsid, write};
use std::ffi::CString;
use std::io::Write;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::io::AsRawFd;

const BUF_SIZE: usize = 256;

#[derive(Debug)]
pub enum InputEvent {
    HotKey(Hotkey),
    Data,
}

#[derive(Debug, Copy, Clone)]
pub enum Hotkey {
    CtrlC,
    CtrlD,
    CtrlB,
}

const CTRL_C: u8 = 3;
const CTRL_D: u8 = 4;
const CTRL_B: u8 = 2;

impl Hotkey {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            CTRL_B => Some(Self::CtrlB),
            CTRL_C => Some(Self::CtrlC),
            CTRL_D => Some(Self::CtrlD),
            _ => None,
        }
    }
}

fn parse_input(input: &[u8], n: usize) -> InputEvent {
    if n == 1 {
        if let Some(k) = Hotkey::from_byte(input[0]) {
            return InputEvent::HotKey(k);
        }
    }
    InputEvent::Data
}

pub struct Session {
    master_fd: OwnedFd,
}

impl Session {
    pub fn new(command: impl Into<String>, args: Vec<String>) -> anyhow::Result<Self> {
        let command = command.into();
        let OpenptyResult { master, slave } = openpty(None, None)?;

        match unsafe { fork()? } {
            ForkResult::Child => {
                drop(master);
                setsid()?;

                let slave_fd = slave.as_raw_fd();
                unsafe {
                    libc::dup2(slave_fd, libc::STDIN_FILENO);
                    libc::dup2(slave_fd, libc::STDOUT_FILENO);
                    libc::dup2(slave_fd, libc::STDERR_FILENO);
                }

                let program = CString::new(command.as_str()).expect("CString::new failed");
                let mut c_args: Vec<CString> = vec![program.clone()];
                for arg in &args {
                    c_args.push(CString::new(arg.as_str()).expect("CString::new failed"));
                }
                let c_args_refs: Vec<&CString> = c_args.iter().collect();

                execvp(program.as_c_str(), &c_args_refs).expect("execvp failed");
                unreachable!();
            }
            ForkResult::Parent { .. } => {
                drop(slave);
                Ok(Self { master_fd: master })
            }
        }
    }

    pub fn master_fd(&self) -> BorrowedFd<'_> {
        self.master_fd.as_fd()
    }

    pub fn write_input(&self, data: &[u8]) -> anyhow::Result<usize> {
        Ok(write(self.master_fd.as_fd(), data)?)
    }

    pub fn read_output(&self, buf: &mut [u8]) -> anyhow::Result<usize> {
        Ok(read(self.master_fd.as_fd(), buf)?)
    }

    pub fn run_input_loop(&self) -> anyhow::Result<()> {
        enable_raw_mode()?;

        let result = self.input_loop_inner();

        disable_raw_mode()?;
        let _ = std::io::stdout().flush();
        let _ = std::io::stdout().write_all(b"\r\n");
        let _ = std::io::stdout().flush();

        result
    }

    fn input_loop_inner(&self) -> anyhow::Result<()> {
        let mut tty_input_buf = [0u8; BUF_SIZE];
        let mut child_output_buf = [0u8; BUF_SIZE];

        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let stdin_fd = stdin.as_fd();
        let master_fd = self.master_fd.as_fd();

        loop {
            let mut fds = FdSet::new();
            fds.insert(stdin_fd);
            fds.insert(master_fd);

            match select(None, Some(&mut fds), None, None, None) {
                Ok(0) => continue,
                Ok(_) => {}
                Err(e) => return Err(e.into()),
            }

            if fds.contains(stdin_fd) {
                let n = read(stdin_fd, &mut tty_input_buf)?;
                if n == 0 {
                    return Ok(());
                }

                match parse_input(&tty_input_buf, n) {
                    InputEvent::Data => {
                        let _ = write(master_fd, &tty_input_buf[..n]);
                    }
                    InputEvent::HotKey(k) => match k {
                        Hotkey::CtrlC | Hotkey::CtrlB => {
                            return Ok(());
                        }
                        Hotkey::CtrlD => {
                            let _ = write(master_fd, &[CTRL_D]);
                        }
                    },
                }
            }

            if fds.contains(master_fd) {
                let n = read(master_fd, &mut child_output_buf)?;
                if n == 0 {
                    return Ok(());
                }
                let _ = write(stdout.as_fd(), &child_output_buf[..n]);
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let session = Session::new("claude", vec![])?;
    session.run_input_loop()
}
