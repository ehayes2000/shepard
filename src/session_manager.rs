use crossterm::{
    QueueableCommand,
    cursor::MoveTo,
    style::Print,
    terminal::{Clear, ClearType, enable_raw_mode, size},
};
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use vt100::{Parser, Screen};

use crate::session::{AttachedSession, DetachedSession, SCROLLBACK, SharedSize};

const BUF_SIZE: usize = 1024;

pub struct SessionManager {
    active_session: Option<AttachedSession>,
    detached_sessions: HashMap<String, DetachedSession>,
    reciever: Receiver<Screen>,
    sender: Sender<Screen>,
    size: SharedSize,
}

impl SessionManager {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let (cols, rows) = size().expect("terminal size");
        let size = SharedSize::new(rows, cols);

        SessionManager {
            active_session: None,
            detached_sessions: HashMap::new(),
            sender: tx,
            reciever: rx,
            size,
        }
    }

    pub fn add_session_active(
        &mut self,
        name: &str,
        command: &str,
        args: &[&str],
    ) -> anyhow::Result<()> {
        let size = self.size.clone();
        let session = AttachedSession::new(name, command, args, self.sender.clone(), size)?;

        if let Some(prev) = self.active_session.take() {
            let detached = prev.detach();
            self.detached_sessions
                .insert(detached.name.to_owned(), detached);
            self.active_session = Some(session);
        } else {
            self.active_session = Some(session);
        }
        Ok(())
    }

    fn switch_to(&mut self, name: &str) -> anyhow::Result<()> {
        // Check if the target session exists in detached sessions
        let mut stdout = std::io::stdout();
        stdout
            .queue(Clear(ClearType::All))
            .expect("clear")
            .queue(MoveTo(0, 0))
            .expect("move to 0 0")
            .flush()
            .expect("flush screen wipe");

        if let Some(detached) = self.detached_sessions.remove(name) {
            // Detach current active session
            if let Some(current) = self.active_session.take() {
                let current_detached = current.detach();
                self.detached_sessions
                    .insert(current_detached.name.to_owned(), current_detached);
            }
            // Attach the new session
            let active = detached.attach().expect("failed to attach");
            let screen = active.get_screen();
            let bytes = &screen.state_formatted();
            stdout.write_all(bytes).expect("show new state");
            stdout.flush().expect("flush new state");
            self.active_session = Some(active);
        }
        Ok(())
    }

    fn switch_to_next(&mut self) -> anyhow::Result<()> {
        // Get the next session name
        if let Some(next_name) = self.detached_sessions.keys().next().cloned() {
            self.switch_to(&next_name)?;
        }
        Ok(())
    }

    pub fn active_session_name(&self) -> Option<&str> {
        self.active_session.as_ref().map(|s| s.name.as_str())
    }

    pub fn session_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.detached_sessions.keys().map(|s| s.as_str()).collect();
        if let Some(ref active) = self.active_session {
            names.push(&active.name);
        }
        names.sort();
        names
    }

    pub fn run(mut self) -> anyhow::Result<()> {
        enable_raw_mode()?;

        let mut stdout = std::io::stdout();
        const CTRL_B: u8 = 0x02;

        let (_, mut reciever) = mpsc::channel();
        std::mem::swap(&mut reciever, &mut self.reciever);
        let shared_size = self.size.clone();

        let shared_self = std::sync::Arc::new(std::sync::Mutex::new(self));
        let shared = shared_self.clone();
        let stdin_thread = std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; BUF_SIZE];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => {
                        break;
                    }
                    Ok(n) => {
                        let mut lock = shared.lock().expect("aquire lock stdin");
                        if n == 1 && buf[0] == CTRL_B {
                            lock.switch_to_next().expect("switch to next");
                            stdout.flush().expect("flush");
                        } else if let Some(session) = &mut lock.active_session {
                            session.write_input(&buf[..n]).expect("write");
                        }
                    }
                    Err(e) => {
                        eprintln!("err! {:#?}", e);
                        break;
                    }
                }
            }
        });

        let shared_size_0 = shared_size.clone();
        let resize_thread = std::thread::spawn(move || {
            let mut signals = Signals::new(&[SIGWINCH]).expect("signal hook");
            for _ in signals.forever() {
                let (cols, rows) = size().expect("terminal size thread");
                shared_size_0.set(rows, cols);
            }
        });

        let stdout_thread = std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            // let mut prev = None::<Screen>;
            loop {
                match reciever.recv() {
                    Ok(mut screen) => {
                        let (rows, cols) = shared_size.get();
                        screen.set_size(rows, cols);
                        // let bytes = if let Some(prev) = prev {
                        //     prev.state_diff(&screen)
                        // } else {
                        //     screen.state_formatted()
                        // };
                        //
                        let bytes = screen.state_formatted();

                        stdout.write_all(&bytes).expect("write state");
                        stdout.flush().expect("flush state");
                        // prev = Some(screen)
                    }
                    Err(e) => {
                        eprintln!("error recieving output {:#?}", e);
                        panic!("error recieving")
                    }
                }
            }
        });

        stdout_thread.join().expect("join stdout");
        stdin_thread.join().expect("join stdin");
        resize_thread.join().expect("join event");
        Ok(())
    }
}
