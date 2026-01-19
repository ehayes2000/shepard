use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use std::collections::HashSet;
use std::path::PathBuf;

pub struct DeleteConfirmDialog {
    worktrees: Vec<PathBuf>,
    active_paths: HashSet<PathBuf>,
}

impl DeleteConfirmDialog {
    pub fn new() -> Self {
        Self {
            worktrees: Vec::new(),
            active_paths: HashSet::new(),
        }
    }

    pub fn set_worktrees_with_active(
        &mut self,
        worktrees: Vec<PathBuf>,
        active_paths: HashSet<PathBuf>,
    ) {
        self.worktrees = worktrees;
        self.active_paths = active_paths;
    }

    pub fn get_worktrees(&self) -> &[PathBuf] {
        &self.worktrees
    }

    pub fn get_active_paths(&self) -> &HashSet<PathBuf> {
        &self.active_paths
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let count = self.worktrees.len();
        let active_count = self
            .worktrees
            .iter()
            .filter(|p| self.active_paths.contains(*p))
            .count();

        let mut lines = vec![Line::from(vec![
            Span::styled(
                "WARNING: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "This action cannot be undone!",
                Style::default().fg(Color::Red),
            ),
        ])];

        // Show active session warning if any
        if active_count > 0 {
            lines.push(Line::from(vec![Span::styled(
                format!(
                    "{} active session{} will be killed!",
                    active_count,
                    if active_count == 1 { "" } else { "s" }
                ),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "Delete {} worktree{}?",
            count,
            if count == 1 { "" } else { "s" }
        )));
        lines.push(Line::from(""));

        // Show worktree paths (limit to 5 to avoid huge dialogs)
        let display_count = self.worktrees.len().min(5);
        for path in self.worktrees.iter().take(display_count) {
            let path_str = path.to_string_lossy();
            let is_active = self.active_paths.contains(path);
            let max_path_len = if is_active { 40 } else { 50 };
            let display = if path_str.len() > max_path_len {
                format!("  ...{}", &path_str[path_str.len() - (max_path_len - 3)..])
            } else {
                format!("  {}", path_str)
            };

            if is_active {
                lines.push(Line::from(vec![
                    Span::styled(display, Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        " [ACTIVE]",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
            } else {
                lines.push(Line::from(Span::styled(
                    display,
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        if self.worktrees.len() > 5 {
            lines.push(Line::from(Span::styled(
                format!("  ... and {} more", self.worktrees.len() - 5),
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "y",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" - Yes, delete permanently"),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" / "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" - Cancel"),
        ]));

        let max_line_len = lines.iter().map(|l| l.width()).max().unwrap_or(30);

        let popup_width = (max_line_len as u16 + 4).min(area.width.saturating_sub(4));
        let popup_height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));

        let popup_x = (area.width.saturating_sub(popup_width)) / 2;
        let popup_y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .title(" Confirm Deletion ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red))
                .style(Style::default().bg(Color::Black)),
        );

        frame.render_widget(paragraph, popup_area);
    }
}

impl Default for DeleteConfirmDialog {
    fn default() -> Self {
        Self::new()
    }
}
