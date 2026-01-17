use crossterm::terminal::size;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::ops::{Deref, DerefMut};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub struct Session {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    active: Arc<Mutex<bool>>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _reader_thread: JoinHandle<()>,
}

pub struct DetachedSession(Session);

impl Deref for DetachedSession {
    type Target = Session;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DetachedSession {
    pub fn attach(self, stdout: &mut impl Write) -> anyhow::Result<AttachedSession> {
        let mut lock = self.0.active.lock().expect("Lock active");
        *lock = true;
        drop(lock);
        // Clear screen
        write!(stdout, "\x1b[2J\x1b[H")?;
        stdout.flush()?;

        // Resize pty to current terminal size
        let (cols, rows) = size().unwrap_or((80, 24));
        self.0.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        Ok(AttachedSession(Session {
            name: self.0.name,
            active: self.0.active,
            command: self.0.command,
            args: self.0.args,
            master: self.0.master,
            writer: self.0.writer,
            child: self.0.child,
            _reader_thread: self.0._reader_thread,
        }))
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
        tx: Sender<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();

        // Get terminal size
        let (cols, rows) = size().unwrap_or((80, 24));
        let pty_size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system.openpty(pty_size)?;

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let active = Arc::new(Mutex::new(true));
        let shared_active = active.clone();

        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let lock = shared_active.lock().expect("reader lock");
                        if !*lock {
                            continue;
                        }
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break; // Receiver dropped
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self(Session {
            name: name.to_string(),
            active,
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            master: pair.master,
            writer,
            child,
            _reader_thread: reader_thread,
        }))
    }

    pub fn detach(self) -> DetachedSession {
        let mut lock = self.0.active.lock().expect("detach lock");
        *lock = false;
        drop(lock);
        DetachedSession(Session {
            name: self.0.name,
            active: self.0.active,
            command: self.0.command,
            args: self.0.args,
            master: self.0.master,
            writer: self.0.writer,
            child: self.0.child,
            _reader_thread: self.0._reader_thread,
        })
    }

    pub fn write_input(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }
}
