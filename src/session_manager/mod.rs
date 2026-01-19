mod session_pair;
mod ui;

pub use ui::{StatusLevel, StatusMessage};
use ui::{CreateDialog, HelpPopup, MainView, SessionPicker, StatusBar};

use crossterm::ExecutableCommand;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, backend::CrosstermBackend};

use std::io::{self, Read, stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use crate::config::Config;
use crate::session::{AttachedSession, SharedSize};
use crate::workflows::{Workflow, WorktreeWorkflow};

use std::sync::mpsc::Sender;

use session_pair::{ActivePair, BackgroundPair, SessionView};

const BUF_SIZE: usize = 1024;
const CTRL_H: u8 = 0x08;
const CTRL_T: u8 = 0x14;
const CTRL_N: u8 = 0x0E;
const CTRL_L: u8 = 0x0C;

fn is_hotkey(bytes: &[u8], key: u8) -> bool {
    bytes.len() == 1 && bytes[0] == key
}

#[derive(Default, Clone, PartialEq)]
enum UiMode {
    #[default]
    Normal,
    HelpPopup,
    ListSessions,
    NewSession,
}

pub struct TuiSessionManager {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    active: Option<ActivePair>,
    background: Vec<BackgroundPair>,
    size: SharedSize,
    mode: UiMode,
    input_rx: Receiver<Vec<u8>>,
    session_counter: usize,
    workflow: Box<dyn Workflow>,
    config: Config,
    startup_path: PathBuf,
    // UI components
    main_view: MainView,
    help_popup: HelpPopup,
    session_picker: SessionPicker,
    create_dialog: CreateDialog,
    status_bar: StatusBar,
    status_tx: Sender<StatusMessage>,
}

impl TuiSessionManager {
    pub fn new() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        stdout().execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::new(backend)?;

        let term_size = terminal.size()?;
        let size = SharedSize::new(
            term_size.height.saturating_sub(2),
            term_size.width.saturating_sub(2),
        );

        let (input_tx, input_rx) = mpsc::channel();

        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; BUF_SIZE];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if input_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let config = Config::load()?;
        let startup_path = std::env::current_dir()?;
        let (status_bar, status_tx) = StatusBar::new();

        Ok(Self {
            terminal,
            active: None,
            background: Vec::new(),
            size,
            mode: UiMode::Normal,
            input_rx,
            session_counter: 0,
            workflow: Box::new(WorktreeWorkflow),
            config,
            startup_path,
            main_view: MainView::new(),
            help_popup: HelpPopup::new(),
            session_picker: SessionPicker::new(),
            create_dialog: CreateDialog::new(),
            status_bar,
            status_tx,
        })
    }

    pub fn status_sender(&self) -> Sender<StatusMessage> {
        self.status_tx.clone()
    }

    fn create_session(
        &self,
        command: &str,
        args: &[&str],
        cwd: &Path,
    ) -> anyhow::Result<AttachedSession> {
        let (tx, _rx) = mpsc::channel();
        AttachedSession::new(command, args, tx, self.size.clone(), Some(cwd))
    }

    pub fn add_claude_session(
        &mut self,
        name: &str,
        command: &str,
        args: &[&str],
        cwd: &Path,
    ) -> anyhow::Result<()> {
        let session = self.create_session(command, args, cwd)?;

        if let Some(old_pair) = self.active.take() {
            self.background.push(old_pair.detach());
        }

        self.active = Some(ActivePair::new(
            name.to_string(),
            cwd.to_path_buf(),
            session,
        ));
        Ok(())
    }

    pub fn new_named_claude_session(&mut self, name: &str) -> anyhow::Result<()> {
        let metadata = match self
            .workflow
            .pre_session_hook(name, &self.config, &self.startup_path)
        {
            Ok(m) => m,
            Err(status_msg) => {
                let _ = self.status_tx.send(status_msg);
                self.mode = UiMode::NewSession;
                return Ok(());
            }
        };

        self.config.set_recent_session(
            self.startup_path.clone(),
            name.to_string(),
            metadata.path.clone(),
        )?;

        let args_owned = self.config.claude_args.clone();
        let args: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
        self.add_claude_session(name, "claude", &args, &metadata.path)
    }

    pub fn try_resume(&mut self) -> anyhow::Result<bool> {
        let recent = match self.config.get_recent_session(&self.startup_path) {
            Some(r) => r.clone(),
            None => return Ok(false),
        };

        if !recent.path.exists() {
            let _ = self.status_tx.send(StatusMessage::err(
                "Resume failed",
                format!("Session path no longer exists: {}", recent.path.display()),
            ));
            return Ok(false);
        }

        let mut args_owned: Vec<String> = vec!["--continue".to_string()];
        args_owned.extend(self.config.claude_args.clone());
        let args: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();

        self.add_claude_session(&recent.name, "claude", &args, &recent.path)?;
        Ok(true)
    }

    pub fn open_new_session(&mut self) {
        self.create_dialog.clear();
        self.mode = UiMode::NewSession;
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        loop {
            let inner_size = self.render_frame()?;
            self.size.set(inner_size.height, inner_size.width);

            match self
                .input_rx
                .recv_timeout(std::time::Duration::from_millis(16))
            {
                Ok(bytes) => match self.mode {
                    UiMode::Normal => self.handle_normal_input(&bytes)?,
                    UiMode::HelpPopup => self.handle_help_input(&bytes)?,
                    UiMode::ListSessions => self.handle_list_input(&bytes)?,
                    UiMode::NewSession => self.handle_new_session_input(&bytes)?,
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok(())
    }

    fn render_frame(&mut self) -> anyhow::Result<ratatui::layout::Rect> {
        // Update status bar (check for new messages, clear expired)
        self.status_bar.update();

        let (screen, active_view) = match &self.active {
            Some(pair) => {
                let screen = match pair.view {
                    SessionView::Claude => pair.claude.get_screen(),
                    SessionView::Shell => pair
                        .shell
                        .as_ref()
                        .map(|s| s.get_screen())
                        .unwrap_or_else(|| pair.claude.get_screen()),
                };
                (Some(screen), pair.view)
            }
            None => (None, SessionView::Claude),
        };
        let active_name = self.active.as_ref().map(|p| p.name.clone());
        let active_path = self.active.as_ref().map(|p| p.path.clone());
        let session_names: Vec<String> = self.background.iter().map(|p| p.name.clone()).collect();
        let background_count = self.background.len();
        let mode = self.mode.clone();

        // Get status bar render data
        let bottom_left = self.status_bar.render_bottom_left();
        let bottom_center = self.status_bar.render_bottom_center();

        let mut inner_area = ratatui::layout::Rect::default();

        self.terminal.draw(|frame| {
            let area = frame.area();

            // Render main view
            inner_area = self.main_view.render(
                frame,
                screen.as_ref(),
                active_name.as_deref(),
                active_path.as_deref(),
                active_view,
                background_count,
                bottom_left,
                bottom_center,
            );

            // Render overlays based on mode
            match mode {
                UiMode::Normal => {}
                UiMode::HelpPopup => {
                    self.help_popup.render(frame, area);
                }
                UiMode::ListSessions => {
                    self.session_picker.render(frame, area, &session_names);
                }
                UiMode::NewSession => {
                    self.create_dialog.render(frame, area);
                }
            }
        })?;

        Ok(inner_area)
    }

    fn handle_normal_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if is_hotkey(bytes, CTRL_H) {
            self.mode = UiMode::HelpPopup;
        } else if is_hotkey(bytes, CTRL_T) {
            self.toggle_shell()?;
        } else if is_hotkey(bytes, CTRL_N) {
            self.create_dialog.clear();
            self.mode = UiMode::NewSession;
        } else if is_hotkey(bytes, CTRL_L) {
            if !self.background.is_empty() {
                self.session_picker.select(Some(0));
                self.mode = UiMode::ListSessions;
            }
        } else if let Some(ref mut pair) = self.active {
            match pair.view {
                SessionView::Claude => pair.claude.write_input(bytes)?,
                SessionView::Shell => {
                    if let Some(ref mut shell) = pair.shell {
                        shell.write_input(bytes)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn toggle_shell(&mut self) -> anyhow::Result<()> {
        let needs_shell = self
            .active
            .as_ref()
            .map(|p| p.view == SessionView::Claude && p.shell.is_none())
            .unwrap_or(false);

        let path = self.active.as_ref().map(|p| p.path.clone());

        if needs_shell {
            if let Some(path) = path {
                let shell_cmd = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
                let shell_session = self.create_session(&shell_cmd, &[], &path)?;
                if let Some(ref mut pair) = self.active {
                    pair.shell = Some(shell_session);
                }
            }
        }

        if let Some(ref mut pair) = self.active {
            match pair.view {
                SessionView::Claude => pair.view = SessionView::Shell,
                SessionView::Shell => pair.view = SessionView::Claude,
            }
        }
        Ok(())
    }

    fn handle_help_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        // Hotkeys work from help popup and close it
        if is_hotkey(bytes, CTRL_H) {
            self.mode = UiMode::Normal;
        } else if is_hotkey(bytes, CTRL_T) {
            self.mode = UiMode::Normal;
            self.toggle_shell()?;
        } else if is_hotkey(bytes, CTRL_N) {
            self.create_dialog.clear();
            self.mode = UiMode::NewSession;
        } else if is_hotkey(bytes, CTRL_L) {
            if !self.background.is_empty() {
                self.session_picker.select(Some(0));
                self.mode = UiMode::ListSessions;
            } else {
                self.mode = UiMode::Normal;
            }
        } else {
            // Any other key just closes help
            self.mode = UiMode::Normal;
        }
        Ok(())
    }

    fn handle_list_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        let session_count = self.background.len();

        if bytes[0] == 0x1b {
            if bytes.len() == 1 {
                self.mode = UiMode::Normal;
                return Ok(());
            }
            if bytes.len() >= 3 && bytes[1] == b'[' {
                match bytes[2] {
                    b'A' => self.session_picker.move_selection(-1, session_count),
                    b'B' => self.session_picker.move_selection(1, session_count),
                    _ => {}
                }
            }
            return Ok(());
        }

        match bytes[0] {
            b'\r' | b'\n' => {
                if let Some(selected) = self.session_picker.selected() {
                    self.switch_to_index(selected)?;
                }
                self.mode = UiMode::Normal;
            }
            b'j' => self.session_picker.move_selection(1, session_count),
            b'k' => self.session_picker.move_selection(-1, session_count),
            b'q' => self.mode = UiMode::Normal,
            _ => {}
        }

        Ok(())
    }

    fn handle_new_session_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        if bytes[0] == 0x1b && bytes.len() == 1 {
            self.create_dialog.clear();
            self.mode = UiMode::Normal;
            return Ok(());
        }

        match bytes[0] {
            b'\r' | b'\n' => {
                let input = self.create_dialog.take_input();
                let name = if input.trim().is_empty() {
                    self.session_counter += 1;
                    format!("claude-{}", self.session_counter)
                } else {
                    input.trim().to_string()
                };
                self.new_named_claude_session(&name)?;
                self.mode = UiMode::Normal;
            }
            0x7f => {
                self.create_dialog.pop();
            }
            b if b.is_ascii_graphic() || b == b' ' => {
                self.create_dialog.push(b as char);
            }
            _ => {}
        }

        Ok(())
    }

    fn switch_to_index(&mut self, index: usize) -> anyhow::Result<()> {
        if index >= self.background.len() {
            return Ok(());
        }

        let bg_pair = self.background.remove(index);

        if let Some(old_pair) = self.active.take() {
            self.background.push(old_pair.detach());
        }

        self.active = Some(bg_pair.attach()?);
        Ok(())
    }
}

impl Drop for TuiSessionManager {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
    }
}
