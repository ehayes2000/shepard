mod session_pair;
mod ui;

pub use ui::StatusMessage;
use ui::{CreateDialog, HelpPopup, KillConfirmDialog, MainView, SessionSelector, StatusBar, TerminalMultiplexer};

use std::collections::HashMap;

use crossterm::ExecutableCommand;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, backend::CrosstermBackend};

use std::io::{self, Read, stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use crate::config::Config;
use crate::history::SessionHistory;
use crate::session::{AttachedSession, SharedSize};
use crate::workflows::{Workflow, WorktreeWorkflow};

use std::sync::mpsc::Sender;

use session_pair::{ActivePair, BackgroundPair, SessionView};

const BUF_SIZE: usize = 1024;

/// Convert an absolute path to a home-relative path string with `~`.
fn path_to_display(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(suffix) = path.strip_prefix(&home) {
            return format!("~/{}", suffix.display());
        }
    path.display().to_string()
}
const CTRL_H: u8 = 0x08;
const CTRL_T: u8 = 0x14;
const CTRL_N: u8 = 0x0E;
const CTRL_L: u8 = 0x0C;
const CTRL_X: u8 = 0x18;
const CTRL_BACKSLASH: u8 = 0x1c;
const CTRL_W: u8 = 0x17;

#[derive(Default, Clone, PartialEq)]
enum UiMode {
    #[default]
    Normal,
    HelpPopup,
    ListSessions,
    NewSession,
    KillConfirmation,
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
    kill_confirm_dialog: KillConfirmDialog,
    status_bar: StatusBar,
    status_tx: Sender<StatusMessage>,
    /// Original active session name when selector opened (for revert on escape)
    selector_original_session: Option<String>,
    /// Cached session list when selector opened (indices stay consistent during preview)
    selector_sessions: Vec<(String, String)>,
    /// Number of live sessions in selector_sessions (rest are history)
    selector_live_count: usize,
    /// Session history for recently used sessions
    history: SessionHistory,
    /// Terminal multiplexers keyed by session name (persists across view switches)
    multiplexers: HashMap<String, TerminalMultiplexer>,
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
        let history = SessionHistory::load().unwrap_or_default();

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
            kill_confirm_dialog: KillConfirmDialog::new(),
            status_bar,
            status_tx,
            selector_original_session: None,
            selector_sessions: Vec::new(),
            selector_live_count: 0,
            history,
            multiplexers: HashMap::new(),
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
        resumed: bool,
    ) -> anyhow::Result<()> {
        let session = self.create_session(command, args, cwd)?;

        if let Some(old_pair) = self.active.take() {
            self.background.push(old_pair.detach());
        }

        self.active = Some(ActivePair::new(
            name.to_string(),
            cwd.to_path_buf(),
            session,
            resumed,
        ));

        // Update history and save immediately
        self.history.touch(name.to_string(), cwd.to_path_buf());
        let _ = self.history.save();

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
        self.add_claude_session(name, "claude", &args, &metadata.path, false)
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

        self.add_claude_session(&recent.name, "claude", &args, &recent.path, true)?;
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
                            UiMode::KillConfirmation => self.handle_kill_confirmation_input(&bytes)?,
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
        // First, clean up dead panes in multiplexers
        self.cleanup_dead_multiplexer_panes();

        // Collect info about dead claude session
        let dead_session_info = if let Some(ref pair) = self.active {
            // Only check claude session death when in Claude view
            if pair.view == SessionView::Claude && pair.claude.is_dead() {
                let error = pair.claude.get_error();
                let log_msg = error.unwrap_or_else(|| "Process exited".to_string());
                let _ = self.status_tx.send(StatusMessage::err(
                    format!("Session {} (claude) died", pair.name),
                    log_msg,
                ));
                Some((pair.name.clone(), pair.path.clone(), pair.resumed))
            } else {
                None
            }
        } else {
            None
        };

        if let Some((name, path, was_resumed)) = dead_session_info {
            // Shutdown and remove the active session
            if let Some(pair) = self.active.take() {
                pair.claude.shutdown();
            }

            // Also cleanup the multiplexer for this session
            if let Some(mut multiplexer) = self.multiplexers.remove(&name) {
                for pane in multiplexer.remove_dead_panes() {
                    pane.shutdown();
                }
                // Shutdown any remaining live panes
                while let Some(pane) = multiplexer.close_active_pane() {
                    pane.shutdown();
                }
            }

            // Close any popups and return to normal mode
            if self.mode == UiMode::ListSessions {
                self.mode = UiMode::Normal;
            }

            // If this was a resumed session, start a fresh session in the same directory
            // without the --continue flag
            if was_resumed {
                let args_owned = self.config.claude_args.clone();
                let args: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
                if let Err(e) = self.add_claude_session(&name, "claude", &args, &path, false) {
                    let _ = self.status_tx.send(StatusMessage::err(
                        "Failed to restart session",
                        format!("{}", e),
                    ));
                } else {
                    let _ = self.status_tx.send(StatusMessage::info(
                        "Session restarted",
                        format!("Started fresh session in {}", path.display()),
                    ));
                }
            }
        }
    }

    /// Clean up dead panes in multiplexers and switch view if needed
    fn cleanup_dead_multiplexer_panes(&mut self) {
        let Some(ref mut pair) = self.active else {
            return;
        };

        if pair.view != SessionView::Shell {
            return;
        }

        let name = pair.name.clone();

        if let Some(multiplexer) = self.multiplexers.get_mut(&name) {
            // Remove and shutdown dead panes
            for dead_pane in multiplexer.remove_dead_panes() {
                dead_pane.shutdown();
            }

            // If all panes are gone, switch back to Claude view
            if multiplexer.is_empty() {
                pair.view = SessionView::Claude;
            }
        }
    }

    /// Handle global hotkeys. Returns true if a hotkey was processed.
    fn handle_hotkey(&mut self, bytes: &[u8]) -> anyhow::Result<bool> {
        // Check if we're in shell view (for shell-specific hotkeys)
        let in_shell_view = self
            .active
            .as_ref()
            .map(|p| p.view == SessionView::Shell)
            .unwrap_or(false);

        // Handle shell-specific hotkeys first (only in Normal mode and Shell view)
        if self.mode == UiMode::Normal && in_shell_view {
            match bytes {
                [b] if *b == CTRL_BACKSLASH => {
                    self.split_shell_pane()?;
                    return Ok(true);
                }
                [b] if *b == CTRL_W => {
                    self.close_shell_pane();
                    return Ok(true);
                }
                [b] if *b == CTRL_H => {
                    self.focus_shell_pane_left();
                    return Ok(true);
                }
                [b] if *b == CTRL_L => {
                    self.focus_shell_pane_right();
                    return Ok(true);
                }
                _ => {}
            }
        }

        // Handle global hotkeys
        let hotkey = match bytes {
            [b] if *b == CTRL_H => CTRL_H,
            [b] if *b == CTRL_T => CTRL_T,
            [b] if *b == CTRL_N => CTRL_N,
            [b] if *b == CTRL_L => CTRL_L,
            [b] if *b == CTRL_X => CTRL_X,
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
            CTRL_X => {
                if self.active.is_some() {
                    if let Some(ref pair) = self.active {
                        self.kill_confirm_dialog.set_session_name(&pair.name);
                    }
                    self.mode = UiMode::KillConfirmation;
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
                    SessionView::Claude => Some(pair.claude.get_screen()),
                    // For shell view, we'll render the multiplexer instead
                    SessionView::Shell => None,
                };
                (screen, pair.view)
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

        // Get multiplexer for shell view rendering (if in shell view)
        let multiplexer_name = if active_view == SessionView::Shell {
            active_name.clone()
        } else {
            None
        };

        self.terminal.draw(|frame| {
            let area = frame.area();

            // Render main view (frame/borders)
            let main_inner = self.main_view.render(
                frame,
                screen.as_ref(),
                active_name.as_deref(),
                active_path.as_deref(),
                active_view,
                background_count,
                bottom_left,
                bottom_center,
            );

            // If in shell view, render the multiplexer inside the frame
            if let Some(ref name) = multiplexer_name {
                if let Some(multiplexer) = self.multiplexers.get(name) {
                    inner_area = multiplexer.render(frame, main_inner);
                } else {
                    inner_area = main_inner;
                }
            } else {
                inner_area = main_inner;
            }

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
                UiMode::KillConfirmation => {
                    self.kill_confirm_dialog.render(frame, area);
                }
            }
        })?;

        Ok(inner_area)
    }

    fn handle_normal_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let Some(ref pair) = self.active else {
            return Ok(());
        };

        let name = pair.name.clone();
        let view = pair.view;

        match view {
            SessionView::Claude => {
                if let Some(ref mut pair) = self.active {
                    if pair.claude.is_dead() {
                        return Ok(());
                    }
                    // Ignore write errors - check_dead_sessions will handle cleanup
                    let _ = pair.claude.write_input(bytes);
                }
            }
            SessionView::Shell => {
                // Route input to the multiplexer's active pane
                if let Some(multiplexer) = self.multiplexers.get_mut(&name)
                    && let Some(pane) = multiplexer.active_pane_mut()
                {
                    if pane.is_dead() {
                        return Ok(());
                    }
                    // Ignore write errors - check_dead_sessions will handle cleanup
                    let _ = pane.write_input(bytes);
                }
            }
        }
        Ok(())
    }

    fn toggle_shell(&mut self) -> anyhow::Result<()> {
        // Get info about current state without holding any borrows
        let (name, path, current_view) = match &self.active {
            Some(pair) => (pair.name.clone(), pair.path.clone(), pair.view),
            None => return Ok(()),
        };

        match current_view {
            SessionView::Claude => {
                // Check if multiplexer needs a pane
                let needs_pane = self
                    .multiplexers
                    .get(&name)
                    .map(|m| m.is_empty())
                    .unwrap_or(true);

                if needs_pane {
                    // Create session first (no borrows held)
                    let shell_cmd = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
                    let shell_session = self.create_session(&shell_cmd, &[], &path)?;

                    // Then add to multiplexer
                    self.multiplexers
                        .entry(name)
                        .or_default()
                        .add_pane(shell_session);
                }

                // Now switch the view
                if let Some(ref mut pair) = self.active {
                    pair.view = SessionView::Shell;
                }
            }
            SessionView::Shell => {
                if let Some(ref mut pair) = self.active {
                    pair.view = SessionView::Claude;
                }
            }
        }
        Ok(())
    }

    /// Split the current shell pane (add a new pane to the multiplexer)
    fn split_shell_pane(&mut self) -> anyhow::Result<()> {
        let Some(ref pair) = self.active else {
            return Ok(());
        };

        if pair.view != SessionView::Shell {
            return Ok(());
        }

        let name = pair.name.clone();
        let path = pair.path.clone();

        let shell_cmd = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let shell_session = self.create_session(&shell_cmd, &[], &path)?;

        if let Some(multiplexer) = self.multiplexers.get_mut(&name) {
            multiplexer.add_pane(shell_session);
        }

        Ok(())
    }

    /// Close the active shell pane (return to Claude view if no panes left)
    fn close_shell_pane(&mut self) {
        let Some(ref mut pair) = self.active else {
            return;
        };

        if pair.view != SessionView::Shell {
            return;
        }

        let name = pair.name.clone();

        if let Some(multiplexer) = self.multiplexers.get_mut(&name) {
            if let Some(closed) = multiplexer.close_active_pane() {
                closed.shutdown();
            }

            // If no panes left, switch back to Claude view
            if multiplexer.is_empty() {
                pair.view = SessionView::Claude;
            }
        }
    }

    /// Focus the pane to the left
    fn focus_shell_pane_left(&mut self) {
        let Some(ref pair) = self.active else {
            return;
        };

        if pair.view != SessionView::Shell {
            return;
        }

        if let Some(multiplexer) = self.multiplexers.get_mut(&pair.name) {
            multiplexer.focus_left();
        }
    }

    /// Focus the pane to the right
    fn focus_shell_pane_right(&mut self) {
        let Some(ref pair) = self.active else {
            return;
        };

        if pair.view != SessionView::Shell {
            return;
        }

        if let Some(multiplexer) = self.multiplexers.get_mut(&pair.name) {
            multiplexer.focus_right();
        }
    }

    fn handle_help_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        // Any non-hotkey key closes help
        if !bytes.is_empty() {
            self.mode = UiMode::Normal;
        }
        Ok(())
    }

    fn handle_kill_confirmation_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        match bytes[0] {
            // Escape key
            0x1b if bytes.len() == 1 => {
                self.mode = UiMode::Normal;
            }
            // 'y' or 'Y' - confirm kill
            b'y' | b'Y' => {
                if let Some(pair) = self.active.take() {
                    let name = pair.name.clone();
                    pair.claude.shutdown();

                    // Also cleanup the multiplexer for this session
                    if let Some(mut multiplexer) = self.multiplexers.remove(&name) {
                        for pane in multiplexer.remove_dead_panes() {
                            pane.shutdown();
                        }
                        // Shutdown any remaining live panes
                        while let Some(pane) = multiplexer.close_active_pane() {
                            pane.shutdown();
                        }
                    }

                    let _ = self.status_tx.send(StatusMessage::info(
                        "Session killed",
                        format!("Killed session '{}'", name),
                    ));
                }
                self.mode = UiMode::Normal;
            }
            // 'n' or 'N' or any other key - cancel
            b'n' | b'N' => {
                self.mode = UiMode::Normal;
            }
            _ => {}
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
        let (sessions, live_count) = self.build_session_list();
        self.selector_sessions = sessions;
        self.selector_live_count = live_count;
        self.session_selector.set_live_count(live_count);
        self.session_selector.update_filter(&self.selector_sessions);
    }

    /// Build session list with live sessions first, then history items.
    /// Returns (list, live_count) where live_count is the number of live sessions.
    fn build_session_list(&self) -> (Vec<(String, String)>, usize) {
        // Collect live sessions first
        let live: Vec<(String, String)> = self.active
            .iter()
            .map(|p| (p.name.clone(), path_to_display(&p.path)))
            .chain(
                self.background
                    .iter()
                    .map(|p| (p.name.clone(), path_to_display(&p.path))),
            )
            .collect();

        let live_count = live.len();

        // Collect history items that aren't currently live
        let live_names: std::collections::HashSet<_> = live.iter()
            .map(|(name, _)| name.as_str())
            .collect();

        let history_items: Vec<(String, String)> = self.history.entries()
            .filter(|entry| !live_names.contains(entry.name.as_str()))
            .map(|entry| (entry.name.clone(), path_to_display(&entry.path)))
            .collect();

        let mut list = live;
        list.extend(history_items);

        (list, live_count)
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
                // Enter - confirm selection
                if self.session_selector.is_selected_history() {
                    // History item selected - try to resume, fallback to new session
                    if let Some(selected) = self.session_selector.selected_original_index()
                        && let Some((name, _)) = self.selector_sessions.get(selected).cloned()
                    {
                        self.start_history_session(&name)?;
                    }
                }
                // For live sessions, we already previewed it, just close
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

    /// Preview the currently selected session (switch to it without closing selector).
    /// Only previews live sessions, not history items.
    fn preview_selected_session(&mut self) -> anyhow::Result<()> {
        // Don't preview history items - they're not live sessions
        if self.session_selector.is_selected_history() {
            return Ok(());
        }

        if let Some(selected) = self.session_selector.selected_original_index() {
            // Get the name from cached session list (indices are stable)
            if let Some((name, _)) = self.selector_sessions.get(selected).cloned() {
                self.switch_to_session_by_name(&name)?;
            }
        }
        Ok(())
    }

    /// Start a session from history - try to resume with --continue, fallback to new session.
    fn start_history_session(&mut self, name: &str) -> anyhow::Result<()> {
        let entry = match self.history.get_by_name(name) {
            Some(e) => e.clone(),
            None => return Ok(()),
        };

        // Check if path still exists
        if !entry.path.exists() {
            let _ = self.status_tx.send(StatusMessage::err(
                "Path not found",
                format!("Session path no longer exists: {}", entry.path.display()),
            ));
            return Ok(());
        }

        // Try to resume with --continue first
        let mut resume_args: Vec<String> = vec!["--continue".to_string()];
        resume_args.extend(self.config.claude_args.clone());
        let args: Vec<&str> = resume_args.iter().map(|s| s.as_str()).collect();

        match self.add_claude_session(&entry.name, "claude", &args, &entry.path, true) {
            Ok(()) => {
                let _ = self.status_tx.send(StatusMessage::info(
                    "Resumed session",
                    format!("Resumed '{}' from history", entry.name),
                ));
            }
            Err(_) => {
                // Resume failed, start fresh session
                let fresh_args_owned = self.config.claude_args.clone();
                let fresh_args: Vec<&str> = fresh_args_owned.iter().map(|s| s.as_str()).collect();
                self.add_claude_session(&entry.name, "claude", &fresh_args, &entry.path, false)?;
                let _ = self.status_tx.send(StatusMessage::info(
                    "New session",
                    format!("Started fresh session '{}' (resume failed)", entry.name),
                ));
            }
        }

        Ok(())
    }

    /// Switch to a session by name, searching both active and background.
    /// Returns true if the session was found and switched to.
    fn switch_to_session_by_name(&mut self, name: &str) -> anyhow::Result<bool> {
        // Check if already active
        if let Some(ref active) = self.active
            && active.name == name {
                return Ok(true);
            }

        // Find in background
        let bg_index = self
            .background
            .iter()
            .position(|p| p.name == name);

        if let Some(idx) = bg_index {
            let bg_pair = self.background.remove(idx);
            let path = bg_pair.path.clone();

            if let Some(old_pair) = self.active.take() {
                self.background.push(old_pair.detach());
            }

            self.active = Some(bg_pair.attach()?);

            // Update history and save immediately
            self.history.touch(name.to_string(), path);
            let _ = self.history.save();

            return Ok(true);
        }

        Ok(false)
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
        // Save history before cleanup
        let _ = self.history.save();

        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
    }
}
