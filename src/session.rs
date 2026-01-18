use arc_swap::ArcSwap;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use vt100::{Callbacks, Parser, Screen};

/// Shared writer for sending responses back to the PTY
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Terminal callbacks that respond to escape sequence queries
pub struct TerminalCallbacks {
    writer: SharedWriter,
}

impl TerminalCallbacks {
    pub fn new(writer: SharedWriter) -> Self {
        Self { writer }
    }

    fn write_response(&mut self, response: &[u8]) {
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.write_all(response);
            let _ = writer.flush();
        }
    }
}

// TODO: this is incomplete + likely wrong
impl Callbacks for TerminalCallbacks {
    fn unhandled_csi(
        &mut self,
        screen: &mut Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        match (i1, c) {
            // CSI 5 n - Device Status Report (operating status)
            // Response: CSI 0 n (terminal OK)
            (None, 'n') if params == [[5]] => {
                self.write_response(b"\x1b[0n");
            }
            // CSI 6 n - Device Status Report (cursor position)
            // Response: CSI row ; col R
            (None, 'n') if params == [[6]] => {
                let (row, col) = screen.cursor_position();
                let response = format!("\x1b[{};{}R", row + 1, col + 1);
                self.write_response(response.as_bytes());
            }
            // CSI c or CSI 0 c - Primary Device Attributes (DA1)
            // Response: VT220 with various capabilities
            (None, 'c') if params.is_empty() || params == [[0]] => {
                // Report as VT220 with ANSI color, etc.
                self.write_response(b"\x1b[?62;1;2;6;22c");
            }
            // CSI > c or CSI > 0 c - Secondary Device Attributes (DA2)
            (Some(b'>'), 'c') if params.is_empty() || params == [[0]] => {
                // Terminal type 0, version 0, ROM version 0
                self.write_response(b"\x1b[>0;0;0c");
            }
            // CSI ? 6 n - DECXCPR (extended cursor position)
            (Some(b'?'), 'n') if params == [[6]] => {
                let (row, col) = screen.cursor_position();
                let response = format!("\x1b[?{};{}R", row + 1, col + 1);
                self.write_response(response.as_bytes());
            }
            _ => {}
        }
    }
}

const SCROLLBACK: usize = 500;
const BUF_SIZE: usize = 8 * 1024;

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
        let cols = (inner & 0xFFFF) as u16;
        (rows, cols)
    }

    pub fn set(&self, rows: u16, cols: u16) {
        self.0.store(Self::pack_size(rows, cols), Ordering::Relaxed);
    }
}

pub struct Session {
    active: Arc<AtomicBool>,
    writer: SharedWriter,
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
        command: &str,
        args: &[&str],
        _tx: Sender<Screen>,
        size: SharedSize,
        cwd: Option<&Path>,
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
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }

        let _ = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer: SharedWriter = Arc::new(Mutex::new(pair.master.take_writer()?));
        let callback_writer = writer.clone();

        let active = Arc::new(AtomicBool::new(true));
        let shared_active = active.clone();

        let screen = Arc::new(ArcSwap::from_pointee(
            Parser::new(rows, cols, SCROLLBACK).screen().clone(),
        ));
        let shared_screen = screen.clone();

        let reader_thread = std::thread::spawn(move || {
            let master = pair.master;
            let callbacks = TerminalCallbacks::new(callback_writer);
            let mut parser = Parser::new_with_callbacks(rows, cols, SCROLLBACK, callbacks);
            let mut buf = [0u8; BUF_SIZE];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        // Check if size changed and update both PTY and parser
                        let (rows, cols) = size.get();
                        let current = master.get_size().expect("get pty size");
                        if current.rows != rows || current.cols != cols {
                            master
                                .resize(PtySize {
                                    rows,
                                    cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                })
                                .expect("resize pty");
                        }
                        parser.screen_mut().set_size(rows, cols);
                        parser.process(&buf[..n]);
                        // Always update the shared screen state
                        shared_screen.store(Arc::new(parser.screen().clone()));
                        let is_active = shared_active.load(Ordering::Acquire);
                        if !is_active {
                            continue;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self(Session {
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
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("lock poisoned"))?;
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    pub fn get_screen(&self) -> Arc<Screen> {
        self.screen.load_full()
    }
}
