//! A session is a program that's running
//! A session may be attached or detached
//! An attached session will take stdin and render to stdout
//! A detached session is still running and uding resources, but isn't attached
//!
//! Threading model
//! # Session
//! A PTY master/slave is created to manage the subprocess
//! A listener thread listens for stdout / stderr from the slave
//!     - update internal terminal state state (vt100::Parser)
//!     - sends terminal state to session manager
//!
//! # SessionManager
//! A stdin thread that listens for stdin and routes stdin to the active Session
//! A state thread that accepts state from all Sessions and displays the state of the active thread
//!
//!
//! ^ This is a lot of threads. Also lots of data being sent from program listener
//! to state monitor. Most data is discarded if lots of programs running.
//!
//! Likely a way to reduce sending lots of redundant data but likely requires a lock.
//! Could a single thread listen for all stdout from all threads?
//! How could new processes be registered? How could old threads be removed?
//! How to prevent race condition / data loss when updating it?
//!
//! Green threads may be better. Register streams to an async state poller running in a green thread
//!  ^ requires that listening for subprocess stdout can be done async. supported by portable-pty?

use arc_swap::ArcSwap;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use vt100::{Parser, Screen};

pub const SCROLLBACK: usize = 1024;
const BUF_SIZE: usize = 1024;

/// (rows, cols) ordered size stored in AtomicU32
#[derive(Clone, Debug)]
pub struct SharedSize(Arc<AtomicU32>);
impl SharedSize {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self(Arc::new(AtomicU32::new(Self::pack_size(rows, cols))))
    }

    fn pack_size(rows: u16, cols: u16) -> u32 {
        ((rows as u32) << 16) | cols as u32
    }

    pub fn get(&self) -> (u16, u16) {
        let inner = self.0.load(Ordering::Relaxed);
        let rows = (inner >> 16) as u16;
        let cols = (inner & 0x00FF) as u16;
        (rows, cols)
    }

    pub fn set(&self, rows: u16, cols: u16) {
        self.0.store(Self::pack_size(rows, cols), Ordering::Relaxed);
    }
}

pub struct Session {
    pub name: String,
    active: Arc<AtomicBool>,
    writer: Box<dyn Write + Send>,
    _reader_thread: JoinHandle<()>,
    screen: Arc<ArcSwap<Screen>>,
}

pub struct DetachedSession(Session);

impl Deref for DetachedSession {
    type Target = Session;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DetachedSession {
    pub fn attach(self) -> anyhow::Result<AttachedSession> {
        self.0.active.store(true, Ordering::Release);
        Ok(AttachedSession(self.0))
    }
}

/// A session that is attached to the terminal - user can interact with it
pub struct AttachedSession(pub Session);

impl Deref for AttachedSession {
    type Target = Session;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AttachedSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl AttachedSession {
    pub fn new(
        name: &str,
        command: &str,
        args: &[&str],
        tx: Sender<Screen>,
        size: SharedSize,
    ) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();

        let (rows, cols) = size.get();
        let pty_size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system.openpty(pty_size)?;

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);

        let _ = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let active = Arc::new(AtomicBool::new(true));
        let shared_active = active.clone();

        let screen = Arc::new(ArcSwap::from_pointee(Parser::new(rows, cols, SCROLLBACK).screen().clone()));
        let shared_screen = screen.clone();

        let reader_thread = std::thread::spawn(move || {
            let master = pair.master;
            let mut parser = Parser::new(rows, cols, SCROLLBACK);
            let mut buf = [0u8; BUF_SIZE];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        // Check if size changed
                        Self::update_size(&*master, &size).expect("update size");
                        parser.write_all(&buf[..n]).expect("write to parser");
                        // Always update the shared screen state
                        shared_screen.store(Arc::new(parser.screen().clone()));
                        let is_active = shared_active.load(Ordering::Acquire);
                        if !is_active {
                            continue;
                        }
                        let screen = parser.screen().to_owned();
                        tx.send(screen).expect("Send screen");
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self(Session {
            name: name.to_string(),
            active,
            writer,
            _reader_thread: reader_thread,
            screen,
        }))
    }

    pub fn detach(self) -> DetachedSession {
        self.0.active.store(false, Ordering::Release);
        DetachedSession(self.0)
    }

    pub fn write_input(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    fn update_size(master: &dyn MasterPty, size: &SharedSize) -> anyhow::Result<()> {
        let (rows, cols) = size.get();
        let size = master.get_size()?;
        if size.rows != rows || size.cols != cols {
            master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;
        }
        Ok(())
    }

    pub fn get_screen(&self) -> Arc<Screen> {
        self.screen.load_full()
    }
}
