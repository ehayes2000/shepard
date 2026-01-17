use crossterm::terminal::{enable_raw_mode, size};
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, Sender};

use crate::config::BUF_SIZE;
use crate::session::{AttachedSession, DetachedSession};

pub struct SessionManager {
    active_session: Option<AttachedSession>,
    detached_sessions: HashMap<String, DetachedSession>,
    reciever: Receiver<Vec<u8>>,
    sender: Sender<Vec<u8>>,
}

impl SessionManager {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        SessionManager {
            active_session: None,
            detached_sessions: HashMap::new(),
            sender: tx,
            reciever: rx,
        }
    }

    pub fn add_session_active(
        &mut self,
        name: &str,
        command: &str,
        args: &[&str],
    ) -> anyhow::Result<()> {
        let session = AttachedSession::new(name, command, args, self.sender.clone())?;

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
        if let Some(detached) = self.detached_sessions.remove(name) {
            // Detach current active session
            if let Some(current) = self.active_session.take() {
                let current_detached = current.detach();
                self.detached_sessions
                    .insert(current_detached.name.to_owned(), current_detached);
            }
            // Attach the new session
            self.active_session = Some(detached.attach(&mut stdout)?);
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
                            stdout.flush().expect("flushg");
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

        let shared = shared_self.clone();

        let resize_thread = std::thread::spawn(move || {
            let mut signals = Signals::new(&[SIGWINCH]).expect("signal hook");
            for _ in signals.forever() {
                let (cols, rows) = size().unwrap_or((80, 24));
                let mut lock = shared.lock().expect("lock");
                if let Some(session) = &mut lock.active_session {
                    let _ = session.resize(cols, rows);
                }
            }
        });

        let stdout_thread = std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            stdout.flush().unwrap();
            loop {
                match reciever.recv() {
                    Ok(bytes) => {
                        stdout.write(bytes.as_slice()).expect("write to stdout");
                        stdout.flush().expect("flush stdout");
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
