mod session_pair;
mod ui;

pub use ui::StatusMessage;
use ui::{CreateDialog, HelpPopup, MainView, SessionSelector, StatusBar};

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

/// Convert an absolute path to a home-relative path string with `~`.
fn path_to_display(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(suffix) = path.strip_prefix(&home) {
            return format!("~/{}", suffix.display());
        }
    }
    path.display().to_string()
}
const CTRL_H: u8 = 0x08;
const CTRL_T: u8 = 0x14;
const CTRL_N: u8 = 0x0E;
const CTRL_L: u8 = 0x0C;

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
    session_selector: SessionSelector,
    create_dialog: CreateDialog,
    status_bar: StatusBar,
    status_tx: Sender<StatusMessage>,
    /// Original active session name when selector opened (for revert on escape)
    selector_original_session: Option<String>,
    /// Cached session list when selector opened (indices stay consistent during preview)
    selector_sessions: Vec<(String, String)>,
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
            session_selector: SessionSelector::new(),
            create_dialog: CreateDialog::new(),
            status_bar,
            status_tx,
            selector_original_session: None,
            selector_sessions: Vec::new(),
        })
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
            // Check for dead sessions before rendering
            self.check_dead_sessions();

            let inner_size = self.render_frame()?;
            self.size.set(inner_size.height, inner_size.width);

            match self
                .input_rx
                .recv_timeout(std::time::Duration::from_millis(16))
            {
                Ok(bytes) => {
                    if !self.handle_hotkey(&bytes)? {
                        match self.mode {
                            UiMode::Normal => self.handle_normal_input(&bytes)?,
                            UiMode::HelpPopup => self.handle_help_input(&bytes)?,
                            UiMode::ListSessions => self.handle_list_input(&bytes)?,
                            UiMode::NewSession => self.handle_new_session_input(&bytes)?,
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok(())
    }

    /// Check if the active session has died and handle cleanup
    fn check_dead_sessions(&mut self) {
        let should_remove = if let Some(ref pair) = self.active {
            // Check if the currently viewed session is dead
            let viewed_dead = match pair.view {
                SessionView::Claude => pair.claude.is_dead(),
                SessionView::Shell => pair.shell.as_ref().map(|s| s.is_dead()).unwrap_or(false),
            };

            if viewed_dead {
                // Get error message from the dead session
                let error = match pair.view {
                    SessionView::Claude => pair.claude.get_error(),
                    SessionView::Shell => pair.shell.as_ref().and_then(|s| s.get_error()),
                };

                let session_name = pair.name.clone();
                let view_name = match pair.view {
                    SessionView::Claude => "claude",
                    SessionView::Shell => "shell",
                };

                let log_msg = error.unwrap_or_else(|| "Process exited".to_string());
                let _ = self.status_tx.send(StatusMessage::err(
                    format!("Session {} ({}) died", session_name, view_name),
                    log_msg,
                ));

                true
            } else {
                false
            }
        } else {
            false
        };

        if should_remove {
            // Shutdown and remove the active session
            if let Some(pair) = self.active.take() {
                pair.claude.shutdown();
                if let Some(ref shell) = pair.shell {
                    shell.shutdown();
                }
            }

            // Close any popups and return to normal mode
            if self.mode == UiMode::ListSessions {
                self.mode = UiMode::Normal;
            }
        }
    }

    /// Handle global hotkeys. Returns true if a hotkey was processed.
    fn handle_hotkey(&mut self, bytes: &[u8]) -> anyhow::Result<bool> {
        let hotkey = match bytes {
            [b] if *b == CTRL_H => CTRL_H,
            [b] if *b == CTRL_T => CTRL_T,
            [b] if *b == CTRL_N => CTRL_N,
            [b] if *b == CTRL_L => CTRL_L,
            _ => return Ok(false),
        };

        // Clean up current mode before switching
        if self.mode == UiMode::NewSession {
            self.create_dialog.clear();
        }

        match hotkey {
            CTRL_H => {
                self.mode = if self.mode == UiMode::HelpPopup {
                    UiMode::Normal
                } else {
                    UiMode::HelpPopup
                };
            }
            CTRL_T => {
                self.mode = UiMode::Normal;
                self.toggle_shell()?;
            }
            CTRL_N => {
                if self.mode != UiMode::NewSession {
                    self.create_dialog.clear();
                    self.mode = UiMode::NewSession;
                }
            }
            CTRL_L => {
                if self.mode == UiMode::ListSessions {
                    self.mode = UiMode::Normal;
                } else if self.active.is_some() || !self.background.is_empty() {
                    self.open_session_selector();
                    self.mode = UiMode::ListSessions;
                } else {
                    self.mode = UiMode::Normal;
                }
            }
            _ => return Ok(false),
        }

        Ok(true)
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
                    self.session_selector.render(frame, area, &self.selector_sessions);
                }
                UiMode::NewSession => {
                    self.create_dialog.render(frame, area);
                }
            }
        })?;

        Ok(inner_area)
    }

    fn handle_normal_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(ref mut pair) = self.active {
            // Check if session is dead before trying to write
            let is_dead = match pair.view {
                SessionView::Claude => pair.claude.is_dead(),
                SessionView::Shell => pair.shell.as_ref().map(|s| s.is_dead()).unwrap_or(true),
            };

            if is_dead {
                // Session is dead, don't try to write - check_dead_sessions will clean up
                return Ok(());
            }

            // Try to write, but don't crash if it fails
            let write_result = match pair.view {
                SessionView::Claude => pair.claude.write_input(bytes),
                SessionView::Shell => {
                    if let Some(ref mut shell) = pair.shell {
                        shell.write_input(bytes)
                    } else {
                        Ok(())
                    }
                }
            };

            // If write failed, the session probably just died - ignore the error
            // check_dead_sessions will handle cleanup on next iteration
            if let Err(_) = write_result {
                return Ok(());
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
        // Any non-hotkey key closes help
        if !bytes.is_empty() {
            self.mode = UiMode::Normal;
        }
        Ok(())
    }

    fn open_session_selector(&mut self) {
        self.session_selector.reset();

        // Save original active session name for revert on escape
        self.selector_original_session = self.active.as_ref().map(|p| p.name.clone());

        // Active session is at index 0 if it exists
        if self.active.is_some() {
            self.session_selector.set_active_index(Some(0));
        }

        // Cache session list (indices remain consistent during preview)
        self.selector_sessions = self.build_session_list();
        self.session_selector.update_filter(&self.selector_sessions);
    }

    fn build_session_list(&self) -> Vec<(String, String)> {
        self.active
            .iter()
            .map(|p| (p.name.clone(), path_to_display(&p.path)))
            .chain(
                self.background
                    .iter()
                    .map(|p| (p.name.clone(), path_to_display(&p.path))),
            )
            .collect()
    }

    fn handle_list_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        // Handle escape sequences (arrows, escape key)
        if bytes[0] == 0x1b {
            if bytes.len() == 1 {
                // Escape key - revert to original session and close
                if let Some(ref original_name) = self.selector_original_session.clone() {
                    self.switch_to_session_by_name(original_name)?;
                }
                self.mode = UiMode::Normal;
                return Ok(());
            }
            if bytes.len() >= 3 && bytes[1] == b'[' {
                match bytes[2] {
                    b'A' => {
                        self.session_selector.move_up();
                        self.preview_selected_session()?;
                    }
                    b'B' => {
                        self.session_selector.move_down();
                        self.preview_selected_session()?;
                    }
                    _ => {}
                }
            }
            return Ok(());
        }

        match bytes[0] {
            b'\r' | b'\n' => {
                // Enter - keep current session (already previewed) and close
                self.mode = UiMode::Normal;
            }
            0x7f => {
                // Backspace - remove character from filter
                self.session_selector.pop_char();
                self.session_selector.update_filter(&self.selector_sessions);
                self.preview_selected_session()?;
            }
            b if b.is_ascii_graphic() || b == b' ' => {
                // Printable character - add to filter
                self.session_selector.push_char(b as char);
                self.session_selector.update_filter(&self.selector_sessions);
                self.preview_selected_session()?;
            }
            _ => {}
        }

        Ok(())
    }

    /// Preview the currently selected session (switch to it without closing selector)
    fn preview_selected_session(&mut self) -> anyhow::Result<()> {
        if let Some(selected) = self.session_selector.selected_original_index() {
            // Get the name from cached session list (indices are stable)
            if let Some((name, _)) = self.selector_sessions.get(selected).cloned() {
                self.switch_to_session_by_name(&name)?;
            }
        }
        Ok(())
    }

    /// Switch to a session by name, searching both active and background
    fn switch_to_session_by_name(&mut self, name: &str) -> anyhow::Result<()> {
        // Check if already active
        if let Some(ref active) = self.active {
            if active.name == name {
                return Ok(());
            }
        }

        // Find in background
        let bg_index = self
            .background
            .iter()
            .position(|p| p.name == name);

        if let Some(idx) = bg_index {
            let bg_pair = self.background.remove(idx);

            if let Some(old_pair) = self.active.take() {
                self.background.push(old_pair.detach());
            }

            self.active = Some(bg_pair.attach()?);
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
}

impl Drop for TuiSessionManager {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
    }
}
