use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders},
};

use crate::pty_widget::PtyWidget;
use crate::session::AttachedSession;

/// Terminal multiplexer managing multiple shell panes
pub struct TerminalMultiplexer {
    panes: Vec<AttachedSession>,
    active_pane: usize,
}

impl TerminalMultiplexer {
    /// Create an empty multiplexer
    pub fn new() -> Self {
        Self {
            panes: Vec::new(),
            active_pane: 0,
        }
    }

    /// Add a new pane and focus it
    pub fn add_pane(&mut self, session: AttachedSession) {
        self.panes.push(session);
        self.active_pane = self.panes.len() - 1;
    }

    /// Close the active pane and return it
    pub fn close_active_pane(&mut self) -> Option<AttachedSession> {
        if self.panes.is_empty() {
            return None;
        }

        let session = self.panes.remove(self.active_pane);

        // Adjust active_pane index
        if self.active_pane >= self.panes.len() && !self.panes.is_empty() {
            self.active_pane = self.panes.len() - 1;
        }

        Some(session)
    }

    /// Focus the pane to the left (wraps around)
    pub fn focus_left(&mut self) {
        if self.panes.is_empty() {
            return;
        }
        if self.active_pane == 0 {
            self.active_pane = self.panes.len() - 1;
        } else {
            self.active_pane -= 1;
        }
    }

    /// Focus the pane to the right (wraps around)
    pub fn focus_right(&mut self) {
        if self.panes.is_empty() {
            return;
        }
        self.active_pane = (self.active_pane + 1) % self.panes.len();
    }

    /// Get mutable reference to the active pane for input
    pub fn active_pane_mut(&mut self) -> Option<&mut AttachedSession> {
        self.panes.get_mut(self.active_pane)
    }

    /// Check if the multiplexer is empty
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    /// Remove dead panes and return them for cleanup
    pub fn remove_dead_panes(&mut self) -> Vec<AttachedSession> {
        let mut dead = Vec::new();
        let mut i = 0;
        while i < self.panes.len() {
            if self.panes[i].is_dead() {
                dead.push(self.panes.remove(i));
                // Adjust active_pane if needed
                if self.active_pane > 0 && self.active_pane >= i {
                    self.active_pane = self.active_pane.saturating_sub(1);
                }
            } else {
                i += 1;
            }
        }
        dead
    }

    /// Render the hotkey bar and horizontal panes, returns the inner area of the panes
    pub fn render(&self, frame: &mut Frame, area: Rect) -> Rect {
        // Split area: 1 row for hotkey bar, rest for panes
        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);

        let hotkey_area = chunks[0];
        let panes_area = chunks[1];

        // Render hotkey bar
        self.render_hotkey_bar(frame, hotkey_area);

        // Render panes
        self.render_panes(frame, panes_area)
    }

    fn render_hotkey_bar(&self, frame: &mut Frame, area: Rect) {
        let hotkeys = Line::from(vec![
            Span::styled(" ^\\", Style::default().fg(Color::Yellow)),
            Span::raw(" Split  "),
            Span::styled("^W", Style::default().fg(Color::Yellow)),
            Span::raw(" Close  "),
            Span::styled("^H", Style::default().fg(Color::Yellow)),
            Span::raw(" Left  "),
            Span::styled("^L", Style::default().fg(Color::Yellow)),
            Span::raw(" Right"),
        ]);

        frame.render_widget(hotkeys, area);
    }

    fn render_panes(&self, frame: &mut Frame, area: Rect) -> Rect {
        if self.panes.is_empty() {
            return area;
        }

        // Create equal-width constraints for each pane
        let constraints: Vec<Constraint> = self
            .panes
            .iter()
            .map(|_| Constraint::Ratio(1, self.panes.len() as u32))
            .collect();

        let pane_areas = Layout::horizontal(constraints).split(area);

        let mut inner_area = Rect::default();

        for (i, pane) in self.panes.iter().enumerate() {
            let is_active = i == self.active_pane;
            let pane_area = pane_areas[i];

            // Create border with different color for active pane
            let border_color = if is_active {
                Color::LightMagenta
            } else {
                Color::White
            };

            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color));

            let inner = block.inner(pane_area);
            frame.render_widget(block, pane_area);

            // Render the terminal content
            let screen = pane.get_screen();
            let mut display_screen = (*screen).clone();

            // Get cursor position before rendering (and potentially resizing)
            let (cursor_row, cursor_col) = display_screen.cursor_position();

            let widget = PtyWidget::new(&mut display_screen);
            frame.render_widget(widget, inner);

            // Position the cursor in the active pane
            if is_active {
                inner_area = inner;
                let cursor_x = inner.x + cursor_col;
                let cursor_y = inner.y + cursor_row;
                // Only set cursor if it's within the visible area
                if cursor_x < inner.x + inner.width && cursor_y < inner.y + inner.height {
                    frame.set_cursor_position((cursor_x, cursor_y));
                }
            }
        }

        inner_area
    }
}

impl Default for TerminalMultiplexer {
    fn default() -> Self {
        Self::new()
    }
}
