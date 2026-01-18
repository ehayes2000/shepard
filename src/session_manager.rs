use crossterm::ExecutableCommand;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
};

use std::io::{self, Read, stdout};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use vt100::Screen;

use crate::pty_widget::PtyWidget;
use crate::session::{AttachedSession, DetachedSession, SharedSize};

const BUF_SIZE: usize = 1024;
const CTRL_K: u8 = 0x0B;

fn is_hotkey(bytes: &[u8]) -> bool {
    bytes.len() == 1 && bytes[0] == CTRL_K
}

pub struct TuiSessionManager {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    active: Option<(String, AttachedSession)>,
    background: Vec<(String, DetachedSession)>,
    size: SharedSize,
    show_picker: bool,
    picker_state: ListState,
    input_rx: Receiver<Vec<u8>>,
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

        Ok(Self {
            terminal,
            active: None,
            background: Vec::new(),
            size,
            show_picker: false,
            picker_state: ListState::default(),
            input_rx,
        })
    }

    pub fn add_session(&mut self, name: &str, command: &str, args: &[&str]) -> anyhow::Result<()> {
        let (tx, _rx) = mpsc::channel::<Screen>();
        let session = AttachedSession::new(command, args, tx, self.size.clone())?;

        if let Some((old_name, old_session)) = self.active.take() {
            self.background.push((old_name, old_session.detach()));
        }

        self.active = Some((name.to_string(), session));
        Ok(())
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
                Ok(bytes) => {
                    if self.show_picker {
                        self.handle_picker_input(&bytes)?;
                    } else if is_hotkey(&bytes) {
                        // Only show picker if there are background sessions
                        if !self.background.is_empty() {
                            self.show_picker = true;
                            self.picker_state.select(Some(0));
                        }
                    } else {
                        if let Some((_, ref mut session)) = self.active {
                            session.write_input(&bytes)?;
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok(())
    }

    fn render_frame(&mut self) -> anyhow::Result<Rect> {
        let screen = self.active.as_ref().map(|(_, s)| s.get_screen());
        let active_name = self.active.as_ref().map(|(n, _)| n.clone());
        // Only show background sessions (not current)
        let session_names: Vec<String> = self.background.iter().map(|(n, _)| n.clone()).collect();
        let background_count = self.background.len();
        let show_picker = self.show_picker;
        let picker_selected = self.picker_state.selected();

        let mut inner_area = Rect::default();

        self.terminal.draw(|frame| {
            inner_area = render(
                frame,
                screen.as_ref(),
                active_name.as_deref(),
                background_count,
                show_picker,
                picker_selected,
                &session_names,
            );
        })?;

        Ok(inner_area)
    }

    fn handle_picker_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        let session_count = self.background.len();

        if bytes[0] == 0x1b {
            if bytes.len() == 1 {
                self.show_picker = false;
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

        // Check for hotkey to close picker
        if is_hotkey(bytes) {
            self.show_picker = false;
            return Ok(());
        }

        match bytes[0] {
            b'\r' | b'\n' => {
                if let Some(selected) = self.picker_state.selected() {
                    self.switch_to_index(selected)?;
                }
                self.show_picker = false;
            }
            b'j' => self.picker_move(1, session_count),
            b'k' => self.picker_move(-1, session_count),
            b'q' => self.show_picker = false,
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

        let (bg_name, bg_session) = self.background.remove(index);

        if let Some((old_name, old_session)) = self.active.take() {
            self.background.push((old_name, old_session.detach()));
        }

        self.active = Some((bg_name, bg_session.attach()?));
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

/// Get the current working directory, relative to home
fn get_pwd() -> String {
    std::env::current_dir()
        .map(|p| path_relative_to_home(&p))
        .unwrap_or_default()
}

fn render(
    frame: &mut Frame,
    screen: Option<&Arc<Screen>>,
    active_name: Option<&str>,
    background_count: usize,
    show_picker: bool,
    picker_selected: Option<usize>,
    session_names: &[String],
) -> Rect {
    let area = frame.area();

    // Build the bottom left status: title + background count (separated by border char)
    let bottom_left = if background_count > 0 {
        format!(
            " {} {} Sessions",
            active_name.unwrap_or(""),
            background_count
        )
    } else {
        format!(" {} ", active_name.unwrap_or(""))
    };

    // Build the bottom right status: pwd relative to home
    let pwd = get_pwd();
    let bottom_right = if pwd.is_empty() {
        String::new()
    } else {
        format!(" {} ", pwd)
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    // Add bottom titles - left aligned title/count, right aligned pwd
    if !bottom_left.trim().is_empty() {
        block = block.title_bottom(Line::from(bottom_left).left_aligned());
    }
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

    if show_picker && !session_names.is_empty() {
        const MAX_VISIBLE: usize = 8;
        let has_overflow = session_names.len() > MAX_VISIBLE;
        let visible_count = session_names.len().min(MAX_VISIBLE);

        // Calculate popup dimensions
        let max_name_len = session_names.iter().map(|s| s.len()).max().unwrap_or(10);
        let popup_width = (max_name_len as u16 + 6)
            .max(24)
            .min(area.width.saturating_sub(4));
        // Height: visible items + 2 for border
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
            " Switch Session (...) "
        } else {
            " Switch Session "
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

    inner
}
