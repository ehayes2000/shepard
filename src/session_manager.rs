use crossterm::ExecutableCommand;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use std::io::{self, Read, stdout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use vt100::Screen;

use crate::config::Config;
use crate::pty_widget::PtyWidget;
use crate::session::{AttachedSession, DetachedSession, SharedSize};
use crate::workflows::{Workflow, WorktreeWorkflow};

const BUF_SIZE: usize = 1024;
const CTRL_K: u8 = 0x0B;

fn is_hotkey(bytes: &[u8]) -> bool {
    bytes.len() == 1 && bytes[0] == CTRL_K
}

#[derive(Default, Clone, PartialEq)]
enum UiMode {
    #[default]
    Normal,
    CommandMenu,
    SelectMode,
    CreateMode,
}

pub struct TuiSessionManager {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    active: Option<(String, PathBuf, AttachedSession)>,
    background: Vec<(String, PathBuf, DetachedSession)>,
    size: SharedSize,
    mode: UiMode,
    picker_state: ListState,
    input_rx: Receiver<Vec<u8>>,
    session_counter: usize,
    create_input: String,
    workflow: Box<dyn Workflow>,
    config: Config,
    startup_path: PathBuf,
}

impl TuiSessionManager {
    pub fn new() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        stdout().execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::new(backend)?;

        let term_size = terminal.size()?;
        // Account for border
        let size = SharedSize::new(
            term_size.height.saturating_sub(2),
            term_size.width.saturating_sub(2),
        );

        let (input_tx, input_rx) = mpsc::channel();

        // Stdin thread - reads raw bytes
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

        Ok(Self {
            terminal,
            active: None,
            background: Vec::new(),
            size,
            mode: UiMode::Normal,
            picker_state: ListState::default(),
            input_rx,
            session_counter: 0,
            create_input: String::new(),
            workflow: Box::new(WorktreeWorkflow),
            config,
            startup_path,
        })
    }

    pub fn add_session(
        &mut self,
        name: &str,
        command: &str,
        args: &[&str],
        cwd: &Path,
    ) -> anyhow::Result<()> {
        let (tx, _rx) = mpsc::channel::<Screen>();
        let session = AttachedSession::new(command, args, tx, self.size.clone(), Some(cwd))?;

        if let Some((old_name, old_path, old_session)) = self.active.take() {
            self.background
                .push((old_name, old_path, old_session.detach()));
        }

        self.active = Some((name.to_string(), cwd.to_path_buf(), session));
        Ok(())
    }

    pub fn new_named_claude_session(&mut self, name: &str) -> anyhow::Result<()> {
        let metadata = self
            .workflow
            .pre_session_hook(name, &self.config, &self.startup_path)?;
        self.config
            .set_recent_session(self.startup_path.clone(), name.to_string(), metadata.path.clone())?;

        let args_owned = self.config.claude_args.clone();
        let args: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
        self.add_session(name, "claude", &args, &metadata.path)
    }

    /// Try to resume a previous session. Returns true if resumed, false if no session to resume.
    pub fn try_resume(&mut self) -> anyhow::Result<bool> {
        let recent = match self.config.get_recent_session(&self.startup_path) {
            Some(r) => r.clone(),
            None => return Ok(false),
        };

        // Build args: --continue plus any configured args
        let mut args_owned: Vec<String> = vec!["--continue".to_string()];
        args_owned.extend(self.config.claude_args.clone());
        let args: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();

        self.add_session(&recent.name, "claude", &args, &recent.path)?;
        Ok(true)
    }

    /// Open the command menu (for when there's no session to resume)
    pub fn open_command_menu(&mut self) {
        self.mode = UiMode::CommandMenu;
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        loop {
            // Render and get inner area size
            let inner_size = self.render_frame()?;

            // Update size for PTY based on render area
            self.size.set(inner_size.height, inner_size.width);

            // Handle input with timeout for refresh
            match self
                .input_rx
                .recv_timeout(std::time::Duration::from_millis(16))
            {
                Ok(bytes) => match self.mode {
                    UiMode::Normal => self.handle_normal_input(&bytes)?,
                    UiMode::CommandMenu => self.handle_command_menu_input(&bytes)?,
                    UiMode::SelectMode => self.handle_select_input(&bytes)?,
                    UiMode::CreateMode => self.handle_create_input(&bytes)?,
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok(())
    }

    fn render_frame(&mut self) -> anyhow::Result<Rect> {
        let screen = self.active.as_ref().map(|(_, _, s)| s.get_screen());
        let active_name = self.active.as_ref().map(|(n, _, _)| n.clone());
        let active_path = self.active.as_ref().map(|(_, p, _)| p.clone());
        let session_names: Vec<String> =
            self.background.iter().map(|(n, _, _)| n.clone()).collect();
        let background_count = self.background.len();
        let mode = self.mode.clone();
        let picker_selected = self.picker_state.selected();
        let create_input = self.create_input.clone();

        let mut inner_area = Rect::default();

        self.terminal.draw(|frame| {
            inner_area = render(
                frame,
                screen.as_ref(),
                active_name.as_deref(),
                active_path.as_deref(),
                background_count,
                &mode,
                picker_selected,
                &session_names,
                &create_input,
            );
        })?;

        Ok(inner_area)
    }

    fn handle_normal_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if is_hotkey(bytes) {
            self.mode = UiMode::CommandMenu;
        } else if let Some((_, _, ref mut session)) = self.active {
            session.write_input(bytes)?;
        }
        Ok(())
    }

    fn handle_command_menu_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        // ESC or Ctrl+K to close
        if bytes[0] == 0x1b || is_hotkey(bytes) {
            self.mode = UiMode::Normal;
            return Ok(());
        }

        match bytes[0] {
            b'c' | b'C' => {
                self.create_input.clear();
                self.mode = UiMode::CreateMode;
            }
            b's' | b'S' => {
                if !self.background.is_empty() {
                    self.picker_state.select(Some(0));
                    self.mode = UiMode::SelectMode;
                }
            }
            b'q' | b'Q' => {
                self.mode = UiMode::Normal;
            }
            _ => {}
        }

        Ok(())
    }

    fn handle_select_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        let session_count = self.background.len();

        // ESC
        if bytes[0] == 0x1b {
            if bytes.len() == 1 {
                self.mode = UiMode::Normal;
                return Ok(());
            }
            if bytes.len() >= 3 && bytes[1] == b'[' {
                match bytes[2] {
                    b'A' => self.picker_move(-1, session_count),
                    b'B' => self.picker_move(1, session_count),
                    _ => {}
                }
            }
            return Ok(());
        }

        // Ctrl+K to go back to command menu
        if is_hotkey(bytes) {
            self.mode = UiMode::CommandMenu;
            return Ok(());
        }

        match bytes[0] {
            b'\r' | b'\n' => {
                if let Some(selected) = self.picker_state.selected() {
                    self.switch_to_index(selected)?;
                }
                self.mode = UiMode::Normal;
            }
            b'j' => self.picker_move(1, session_count),
            b'k' => self.picker_move(-1, session_count),
            b'q' => self.mode = UiMode::Normal,
            _ => {}
        }

        Ok(())
    }

    fn handle_create_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        // ESC to cancel
        if bytes[0] == 0x1b && bytes.len() == 1 {
            self.create_input.clear();
            self.mode = UiMode::Normal;
            return Ok(());
        }

        // Ctrl+K to go back to command menu
        if is_hotkey(bytes) {
            self.create_input.clear();
            self.mode = UiMode::CommandMenu;
            return Ok(());
        }

        match bytes[0] {
            b'\r' | b'\n' => {
                // Submit
                let name = if self.create_input.trim().is_empty() {
                    self.session_counter += 1;
                    format!("claude-{}", self.session_counter)
                } else {
                    self.create_input.trim().to_string()
                };
                self.new_named_claude_session(&name)?;
                self.create_input.clear();
                self.mode = UiMode::Normal;
            }
            0x7f | 0x08 => {
                // Backspace
                self.create_input.pop();
            }
            b if b.is_ascii_graphic() || b == b' ' => {
                self.create_input.push(b as char);
            }
            _ => {}
        }

        Ok(())
    }

    fn picker_move(&mut self, delta: i32, count: usize) {
        if count == 0 {
            return;
        }
        let current = self.picker_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(count as i32) as usize;
        self.picker_state.select(Some(next));
    }

    fn switch_to_index(&mut self, index: usize) -> anyhow::Result<()> {
        if index >= self.background.len() {
            return Ok(());
        }

        let (bg_name, bg_path, bg_session) = self.background.remove(index);

        if let Some((old_name, old_path, old_session)) = self.active.take() {
            self.background
                .push((old_name, old_path, old_session.detach()));
        }

        self.active = Some((bg_name, bg_path, bg_session.attach()?));
        Ok(())
    }
}

impl Drop for TuiSessionManager {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
    }
}

/// Convert an absolute path to be relative to home directory (using ~)
fn path_relative_to_home(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(relative) = path.strip_prefix(&home) {
            return format!("~/{}", relative.display());
        }
    }
    path.display().to_string()
}

fn render(
    frame: &mut Frame,
    screen: Option<&Arc<Screen>>,
    active_name: Option<&str>,
    active_path: Option<&Path>,
    background_count: usize,
    mode: &UiMode,
    picker_selected: Option<usize>,
    session_names: &[String],
    create_input: &str,
) -> Rect {
    let area = frame.area();

    // Top title: session name (left-aligned)
    let top_title = format!(" {} ", active_name.unwrap_or(""));

    // Bottom left: total session count (including active)
    let total_sessions = background_count + if active_name.is_some() { 1 } else { 0 };
    let bottom_left = if total_sessions > 1 {
        format!(" {} Sessions ", total_sessions)
    } else {
        String::new()
    };

    // Bottom center: hotkey hint (styled like command menu keys)
    let bottom_center = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "ctrl+k",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ]);

    // Bottom right: session path relative to home
    let bottom_right = active_path
        .map(|p| format!(" {} ", path_relative_to_home(p)))
        .unwrap_or_default();

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Line::from(top_title).left_aligned());

    // Add bottom titles
    if !bottom_left.is_empty() {
        block = block.title_bottom(Line::from(bottom_left).left_aligned());
    }
    block = block.title_bottom(bottom_center.centered());
    if !bottom_right.is_empty() {
        block = block.title_bottom(Line::from(bottom_right).right_aligned());
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(screen) = screen {
        let mut display_screen = (**screen).clone();
        let widget = PtyWidget::new(&mut display_screen);
        frame.render_widget(widget, inner);
    }

    // Render overlays based on mode
    match mode {
        UiMode::Normal => {}
        UiMode::CommandMenu => {
            render_command_menu(frame, area);
        }
        UiMode::SelectMode => {
            render_session_picker(frame, area, picker_selected, session_names);
        }
        UiMode::CreateMode => {
            render_create_dialog(frame, area, create_input);
        }
    }

    inner
}

fn render_command_menu(frame: &mut Frame, area: Rect) {
    let popup_width = 30u16;
    let popup_height = 6u16;

    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let items = vec![
        ListItem::new(Line::from(vec![
            Span::styled(
                "c",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  Create session"),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled(
                "s",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  Select session"),
        ])),
    ];

    let list = List::new(items).block(
        Block::default()
            .title(" Command Menu ")
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Black)),
    );

    frame.render_widget(list, popup_area);
}

fn render_session_picker(
    frame: &mut Frame,
    area: Rect,
    picker_selected: Option<usize>,
    session_names: &[String],
) {
    if session_names.is_empty() {
        return;
    }

    const MAX_VISIBLE: usize = 8;
    let has_overflow = session_names.len() > MAX_VISIBLE;
    let visible_count = session_names.len().min(MAX_VISIBLE);

    let max_name_len = session_names.iter().map(|s| s.len()).max().unwrap_or(10);
    let popup_width = (max_name_len as u16 + 6)
        .max(24)
        .min(area.width.saturating_sub(4));
    let popup_height = (visible_count as u16 + 2).min(area.height.saturating_sub(2));

    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = session_names
        .iter()
        .map(|name| ListItem::new(name.as_str()))
        .collect();

    let mut list_state = ListState::default();
    list_state.select(picker_selected);

    let title = if has_overflow {
        " Select Session (...) "
    } else {
        " Select Session "
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .style(Style::default().bg(Color::Black)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

fn render_create_dialog(frame: &mut Frame, area: Rect, input: &str) {
    let popup_width = 40u16;
    let popup_height = 5u16;

    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Create Session ")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Input line with cursor
    let display_text = if input.is_empty() {
        Line::from(vec![
            Span::styled("Name: ", Style::default().fg(Color::Gray)),
            Span::styled("_", Style::default().fg(Color::Yellow)),
        ])
    } else {
        Line::from(vec![
            Span::styled("Name: ", Style::default().fg(Color::Gray)),
            Span::raw(input),
            Span::styled("_", Style::default().fg(Color::Yellow)),
        ])
    };

    let paragraph = Paragraph::new(display_text);
    frame.render_widget(paragraph, inner);
}
