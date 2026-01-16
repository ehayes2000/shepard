use std::io::{self, Read, Write as IoWrite};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const TOGGLE_KEY: char = 'b';

struct Session {
    name: String,
    worktree_path: PathBuf,
    pty_master: Box<dyn portable_pty::MasterPty + Send>,
    pty_writer: Box<dyn IoWrite + Send>,
    output_receiver: Receiver<Vec<u8>>,
    alive: bool,
}

impl Session {
    fn write_input(&mut self, data: &[u8]) {
        let _ = self.pty_writer.write_all(data);
        let _ = self.pty_writer.flush();
    }

    fn resize(&self, cols: u16, rows: u16) {
        let _ = self.pty_master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

struct App {
    sessions: Vec<Session>,
    active_session: Option<usize>,
    selected_index: usize,
    show_overlay: bool,
    input_mode: InputMode,
    input_buffer: String,
    repo_path: PathBuf,
    repo_name: String,
    worktrees_dir: PathBuf,
    status_message: String,
    should_quit: bool,
    terminal_size: (u16, u16),
}

#[derive(PartialEq)]
enum InputMode {
    Normal,
    CreatingSession,
}

impl App {
    fn new(terminal_size: (u16, u16)) -> Result<Self> {
        let repo_path = std::env::current_dir().context("Failed to get current directory")?;

        let repo_name = repo_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("repo")
            .to_string();

        let worktrees_dir = std::env::var("WORKTREES")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join("worktrees"));

        if !worktrees_dir.exists() {
            std::fs::create_dir_all(&worktrees_dir)?;
        }

        Ok(Self {
            sessions: Vec::new(),
            active_session: None,
            selected_index: 0,
            show_overlay: true,
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            repo_path,
            repo_name,
            worktrees_dir,
            status_message: String::from(
                "n: new | Enter: switch | d: delete | Ctrl+b: toggle | q: quit",
            ),
            should_quit: false,
            terminal_size,
        })
    }

    fn create_session(&mut self, name: String) -> Result<()> {
        let branch = &name;
        let worktree_name = format!("{}-{}", self.repo_name, name);
        let worktree_path = self.worktrees_dir.join(&worktree_name);

        self.status_message = format!("Creating worktree '{}'...", worktree_name);

        // Create git worktree
        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
            ])
            .current_dir(&self.repo_path)
            .output();

        let worktree_ok = match output {
            Ok(out) if out.status.success() => true,
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if stderr.contains("already exists") {
                    let output2 = Command::new("git")
                        .args(["worktree", "add", worktree_path.to_str().unwrap(), branch])
                        .current_dir(&self.repo_path)
                        .output();
                    matches!(output2, Ok(out2) if out2.status.success()) || worktree_path.exists()
                } else {
                    self.status_message = format!("Failed: {}", stderr.trim());
                    return Ok(());
                }
            }
            Err(e) => {
                self.status_message = format!("Failed to run git: {}", e);
                return Ok(());
            }
        };

        if !worktree_ok && !worktree_path.exists() {
            self.status_message = "Failed to create worktree".to_string();
            return Ok(());
        }

        // Create PTY
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: self.terminal_size.1,
                cols: self.terminal_size.0,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY")?;

        let mut cmd = CommandBuilder::new("claude");
        cmd.cwd(&worktree_path);

        pty_pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn claude")?;

        // Reader thread for PTY output
        let mut reader = pty_pair.master.try_clone_reader().unwrap();
        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let writer = pty_pair.master.take_writer().unwrap();

        let session = Session {
            name: name.clone(),
            worktree_path,
            pty_master: pty_pair.master,
            pty_writer: writer,
            output_receiver: rx,
            alive: true,
        };

        self.sessions.push(session);
        let idx = self.sessions.len() - 1;
        self.active_session = Some(idx);
        self.selected_index = idx;
        self.show_overlay = false;
        self.status_message = format!("Session '{}' created", name);

        Ok(())
    }

    fn switch_to_session(&mut self, index: usize) {
        if index < self.sessions.len() {
            self.active_session = Some(index);
            self.show_overlay = false;
            // Resize the PTY to current terminal size
            self.sessions[index].resize(self.terminal_size.0, self.terminal_size.1);
        }
    }

    fn delete_session(&mut self, index: usize) {
        if index >= self.sessions.len() {
            return;
        }

        let session = &self.sessions[index];
        let name = session.name.clone();
        let worktree_path = session.worktree_path.clone();

        let _ = Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                worktree_path.to_str().unwrap(),
            ])
            .current_dir(&self.repo_path)
            .output();

        self.sessions.remove(index);

        if self.sessions.is_empty() {
            self.active_session = None;
            self.selected_index = 0;
        } else {
            self.selected_index = self.selected_index.min(self.sessions.len() - 1);
            if self.active_session == Some(index) {
                self.active_session = Some(self.selected_index);
            } else if let Some(active) = self.active_session {
                if active > index {
                    self.active_session = Some(active - 1);
                }
            }
        }

        self.status_message = format!("Session '{}' deleted", name);
    }

    fn select_next(&mut self) {
        if !self.sessions.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.sessions.len();
        }
    }

    fn select_previous(&mut self) {
        if !self.sessions.is_empty() {
            self.selected_index = if self.selected_index == 0 {
                self.sessions.len() - 1
            } else {
                self.selected_index - 1
            };
        }
    }
}

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;

    let size = terminal::size()?;
    let mut app = App::new(size)?;

    let result = run_app(&mut app, &mut stdout);

    // Cleanup
    disable_raw_mode()?;
    stdout.execute(LeaveAlternateScreen)?;

    result
}

fn run_app(app: &mut App, stdout: &mut io::Stdout) -> Result<()> {
    loop {
        // Drain PTY output and write to stdout if session active and overlay hidden
        if !app.show_overlay {
            if let Some(idx) = app.active_session {
                let session = &mut app.sessions[idx];
                loop {
                    match session.output_receiver.try_recv() {
                        Ok(data) => {
                            stdout.write_all(&data)?;
                            stdout.flush()?;
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            session.alive = false;
                            break;
                        }
                    }
                }
            }
        } else {
            // Drain but don't display while overlay is shown
            for session in &mut app.sessions {
                while session.output_receiver.try_recv().is_ok() {}
            }
        }

        // Draw overlay if visible
        if app.show_overlay {
            draw_overlay(stdout, app)?;
        }

        // Handle input
        if event::poll(Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // Ctrl+B always toggles overlay
                    if key.code == KeyCode::Char(TOGGLE_KEY)
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        app.show_overlay = !app.show_overlay;
                        if !app.show_overlay {
                            // Clear screen and let PTY redraw
                            stdout.execute(Clear(ClearType::All))?;
                            stdout.execute(MoveTo(0, 0))?;
                            // Send SIGWINCH-like resize to trigger redraw
                            if let Some(idx) = app.active_session {
                                let (cols, rows) = app.terminal_size;
                                app.sessions[idx].resize(cols, rows);
                            }
                        }
                        continue;
                    }

                    if app.show_overlay {
                        handle_overlay_input(app, key.code, key.modifiers, stdout)?;
                    } else if let Some(idx) = app.active_session {
                        // Pass input to active session
                        let bytes = key_to_bytes(key.code, key.modifiers);
                        if !bytes.is_empty() {
                            app.sessions[idx].write_input(&bytes);
                        }
                    }
                }
                Event::Resize(cols, rows) => {
                    app.terminal_size = (cols, rows);
                    if let Some(idx) = app.active_session {
                        app.sessions[idx].resize(cols, rows);
                    }
                }
                Event::Paste(text) => {
                    if !app.show_overlay {
                        if let Some(idx) = app.active_session {
                            app.sessions[idx].write_input(text.as_bytes());
                        }
                    }
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_overlay_input(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    stdout: &mut io::Stdout,
) -> Result<()> {
    match app.input_mode {
        InputMode::Normal => match code {
            KeyCode::Char('q') => app.should_quit = true,
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.should_quit = true;
            }
            KeyCode::Char('n') => {
                app.input_mode = InputMode::CreatingSession;
                app.input_buffer.clear();
                app.status_message = "Enter branch/session name:".to_string();
            }
            KeyCode::Char('d') => {
                if !app.sessions.is_empty() {
                    app.delete_session(app.selected_index);
                }
            }
            KeyCode::Enter => {
                if !app.sessions.is_empty() {
                    let idx = app.selected_index;
                    app.switch_to_session(idx);
                    stdout.execute(Clear(ClearType::All))?;
                    stdout.execute(MoveTo(0, 0))?;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => app.select_next(),
            KeyCode::Up | KeyCode::Char('k') => app.select_previous(),
            KeyCode::Esc => {
                if app.active_session.is_some() {
                    app.show_overlay = false;
                    stdout.execute(Clear(ClearType::All))?;
                    stdout.execute(MoveTo(0, 0))?;
                    if let Some(idx) = app.active_session {
                        let (cols, rows) = app.terminal_size;
                        app.sessions[idx].resize(cols, rows);
                    }
                }
            }
            _ => {}
        },
        InputMode::CreatingSession => match code {
            KeyCode::Enter => {
                if !app.input_buffer.is_empty() {
                    let name = app.input_buffer.clone();
                    app.input_buffer.clear();
                    app.input_mode = InputMode::Normal;
                    app.create_session(name)?;
                    if !app.show_overlay {
                        stdout.execute(Clear(ClearType::All))?;
                        stdout.execute(MoveTo(0, 0))?;
                    }
                }
            }
            KeyCode::Esc => {
                app.input_mode = InputMode::Normal;
                app.input_buffer.clear();
                app.status_message =
                    "n: new | Enter: switch | d: delete | Ctrl+b: toggle | q: quit".to_string();
            }
            KeyCode::Backspace => {
                app.input_buffer.pop();
            }
            KeyCode::Char(c) => {
                app.input_buffer.push(c);
            }
            _ => {}
        },
    }
    Ok(())
}

fn key_to_bytes(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
    match code {
        KeyCode::Char(c) => {
            if modifiers.contains(KeyModifiers::CONTROL) {
                vec![(c as u8) & 0x1f]
            } else {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![27],
        KeyCode::Up => vec![27, b'[', b'A'],
        KeyCode::Down => vec![27, b'[', b'B'],
        KeyCode::Right => vec![27, b'[', b'C'],
        KeyCode::Left => vec![27, b'[', b'D'],
        KeyCode::Home => vec![27, b'[', b'H'],
        KeyCode::End => vec![27, b'[', b'F'],
        KeyCode::PageUp => vec![27, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![27, b'[', b'6', b'~'],
        KeyCode::Delete => vec![27, b'[', b'3', b'~'],
        KeyCode::Insert => vec![27, b'[', b'2', b'~'],
        KeyCode::F(n) => match n {
            1 => vec![27, b'O', b'P'],
            2 => vec![27, b'O', b'Q'],
            3 => vec![27, b'O', b'R'],
            4 => vec![27, b'O', b'S'],
            5..=12 => {
                let codes = [15, 17, 18, 19, 20, 21, 23, 24];
                let idx = (n - 5) as usize;
                if idx < codes.len() {
                    format!("\x1b[{}~", codes[idx]).into_bytes()
                } else {
                    vec![]
                }
            }
            _ => vec![],
        },
        _ => vec![],
    }
}

fn draw_overlay(stdout: &mut io::Stdout, app: &App) -> Result<()> {
    let (cols, rows) = app.terminal_size;

    // Calculate overlay dimensions
    let overlay_width = 50.min(cols.saturating_sub(4)) as usize;
    let overlay_height = (app.sessions.len() + 6).min(rows.saturating_sub(4) as usize);
    let start_x = ((cols as usize).saturating_sub(overlay_width)) / 2;
    let start_y = ((rows as usize).saturating_sub(overlay_height)) / 2;

    // Draw box
    stdout.execute(Hide)?;

    // Top border
    stdout.execute(MoveTo(start_x as u16, start_y as u16))?;
    stdout.execute(SetBackgroundColor(Color::Black))?;
    stdout.execute(SetForegroundColor(Color::Cyan))?;
    stdout.execute(Print(format!(
        "┌{:─<width$}┐",
        "",
        width = overlay_width - 2
    )))?;

    // Title
    stdout.execute(MoveTo(start_x as u16, (start_y + 1) as u16))?;
    let title = " Shepard ";
    let padding = (overlay_width - 2 - title.len()) / 2;
    stdout.execute(Print(format!(
        "│{:padding$}{}{:rest$}│",
        "",
        title,
        "",
        padding = padding,
        rest = overlay_width - 2 - padding - title.len()
    )))?;

    // Separator
    stdout.execute(MoveTo(start_x as u16, (start_y + 2) as u16))?;
    stdout.execute(Print(format!(
        "├{:─<width$}┤",
        "",
        width = overlay_width - 2
    )))?;

    // Sessions
    let session_area_start = start_y + 3;
    for (i, session) in app.sessions.iter().enumerate() {
        stdout.execute(MoveTo(start_x as u16, (session_area_start + i) as u16))?;

        let marker = if Some(i) == app.active_session {
            "*"
        } else {
            " "
        };
        let selected = if i == app.selected_index { ">" } else { " " };
        let status = if session.alive { "●" } else { "○" };

        if i == app.selected_index {
            stdout.execute(SetBackgroundColor(Color::DarkGrey))?;
        } else {
            stdout.execute(SetBackgroundColor(Color::Black))?;
        }

        if session.alive {
            stdout.execute(SetForegroundColor(Color::Green))?;
        } else {
            stdout.execute(SetForegroundColor(Color::Red))?;
        }

        let content = format!("{}{} {} {}", selected, marker, status, session.name);
        let padded = format!("│ {:<width$} │", content, width = overlay_width - 4);
        stdout.execute(Print(&padded[..padded.len().min(overlay_width + 2)]))?;
    }

    // Empty message if no sessions
    if app.sessions.is_empty() {
        stdout.execute(MoveTo(start_x as u16, session_area_start as u16))?;
        stdout.execute(SetBackgroundColor(Color::Black))?;
        stdout.execute(SetForegroundColor(Color::DarkGrey))?;
        let msg = "(no sessions - press 'n')";
        let padded = format!("│ {:^width$} │", msg, width = overlay_width - 4);
        stdout.execute(Print(padded))?;
    }

    // Separator before status
    let status_y = session_area_start + app.sessions.len().max(1);
    stdout.execute(MoveTo(start_x as u16, status_y as u16))?;
    stdout.execute(SetForegroundColor(Color::Cyan))?;
    stdout.execute(Print(format!(
        "├{:─<width$}┤",
        "",
        width = overlay_width - 2
    )))?;

    // Status line
    stdout.execute(MoveTo(start_x as u16, (status_y + 1) as u16))?;
    stdout.execute(SetForegroundColor(Color::White))?;
    let status = match app.input_mode {
        InputMode::Normal => app.status_message.clone(),
        InputMode::CreatingSession => format!("Branch: {}█", app.input_buffer),
    };
    let status_display: String = status.chars().take(overlay_width - 4).collect();
    let padded = format!("│ {:<width$} │", status_display, width = overlay_width - 4);
    stdout.execute(Print(padded))?;

    // Bottom border
    stdout.execute(MoveTo(start_x as u16, (status_y + 2) as u16))?;
    stdout.execute(SetForegroundColor(Color::Cyan))?;
    stdout.execute(Print(format!(
        "└{:─<width$}┘",
        "",
        width = overlay_width - 2
    )))?;

    stdout.execute(ResetColor)?;
    stdout.execute(Show)?;
    stdout.flush()?;

    Ok(())
}
