use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

const MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StatusLevel {
    Err,
}

#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub level: StatusLevel,
    pub display_message: String,
    pub log_message: String,
}

impl StatusMessage {
    pub fn new(
        level: StatusLevel,
        display_message: impl Into<String>,
        log_message: impl Into<String>,
    ) -> Self {
        Self {
            level,
            display_message: display_message.into(),
            log_message: log_message.into(),
        }
    }

    pub fn err(display: impl Into<String>, log: impl Into<String>) -> Self {
        Self::new(StatusLevel::Err, display, log)
    }
}

struct ActiveMessage {
    message: StatusMessage,
    received_at: Instant,
}

pub struct StatusBar {
    rx: Receiver<StatusMessage>,
    current: Option<ActiveMessage>,
    event_log: EventLog,
}

impl StatusBar {
    pub fn new() -> (Self, Sender<StatusMessage>) {
        let (tx, rx) = mpsc::channel();
        let event_log = EventLog::new();
        (
            Self {
                rx,
                current: None,
                event_log,
            },
            tx,
        )
    }

    pub fn update(&mut self) {
        // Check for new messages
        while let Ok(msg) = self.rx.try_recv() {
            self.event_log.append(&msg);
            self.current = Some(ActiveMessage {
                message: msg,
                received_at: Instant::now(),
            });
        }

        // Clear expired messages
        if let Some(ref active) = self.current {
            if active.received_at.elapsed() >= MESSAGE_TIMEOUT {
                self.current = None;
            }
        }
    }

    pub fn render_bottom_left(&self) -> Line<'static> {
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "ctrl+k",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                "ctrl+t",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ])
    }

    pub fn render_bottom_center(&self) -> Option<Line<'static>> {
        self.current.as_ref().map(|active| {
            let style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);

            Line::from(vec![
                Span::raw(" "),
                Span::styled(active.message.display_message.clone(), style),
                Span::raw(" "),
            ])
        })
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new().0
    }
}

const MAX_LOG_LINES: usize = 1000;

struct EventLog {
    path: Option<std::path::PathBuf>,
}

impl EventLog {
    fn new() -> Self {
        let path = dirs::home_dir().map(|h| h.join(".shepard").join("events.log"));
        Self { path }
    }

    fn append(&mut self, msg: &StatusMessage) {
        let Some(ref path) = self.path else {
            return;
        };

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Read existing entries
        let mut entries: Vec<String> = if path.exists() {
            std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        };

        // Create new entry with timestamp and level
        let level_str = match msg.level {
            StatusLevel::Err => "ERR",
        };
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let entry = format!("[{}] [{}] {}", timestamp, level_str, msg.log_message);
        entries.push(entry);

        // Keep only the most recent entries
        if entries.len() > MAX_LOG_LINES {
            entries = entries.split_off(entries.len() - MAX_LOG_LINES);
        }

        // Write back
        let _ = std::fs::write(path, entries.join("\n") + "\n");
    }
}
