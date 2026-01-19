use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
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
            Span::styled(" ^\\", Style::default().fg(Color::Magenta)),
            Span::raw(" Split  "),
            Span::styled("^W", Style::default().fg(Color::Magenta)),
            Span::raw(" Close  "),
            Span::styled("^H", Style::default().fg(Color::Magenta)),
            Span::raw(" Left  "),
            Span::styled("^L", Style::default().fg(Color::Magenta)),
            Span::raw(" Right"),
        ]);

        frame.render_widget(hotkeys, area);
    }

    fn render_panes(&self, frame: &mut Frame, area: Rect) -> Rect {
        if self.panes.is_empty() {
            return area;
        }

        // Single pane: no dividers needed
        if self.panes.len() == 1 {
            let pane = &self.panes[0];
            let screen = pane.get_screen();
            let mut display_screen = (*screen).clone();
            let (cursor_row, cursor_col) = display_screen.cursor_position();

            let widget = PtyWidget::new(&mut display_screen);
            frame.render_widget(widget, area);

            let cursor_x = area.x + cursor_col;
            let cursor_y = area.y + cursor_row;
            if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
                frame.set_cursor_position((cursor_x, cursor_y));
            }
            return area;
        }

        // Multiple panes: create constraints with dividers between them
        // Pattern: [Pane, Divider, Pane, Divider, ..., Pane]
        let num_panes = self.panes.len();
        let num_dividers = num_panes - 1;
        let total_divider_width = num_dividers as u16;
        let pane_width = area.width.saturating_sub(total_divider_width) / num_panes as u16;

        let mut constraints = Vec::with_capacity(num_panes + num_dividers);
        for i in 0..num_panes {
            constraints.push(Constraint::Length(pane_width));
            if i < num_panes - 1 {
                constraints.push(Constraint::Length(1)); // Divider
            }
        }

        let chunks = Layout::horizontal(constraints).split(area);

        let mut inner_area = Rect::default();
        let divider_style = Style::default().fg(Color::White);

        for (i, pane) in self.panes.iter().enumerate() {
            let is_active = i == self.active_pane;
            // Pane areas are at even indices (0, 2, 4, ...)
            let pane_area = chunks[i * 2];

            // Render the terminal content
            let screen = pane.get_screen();
            let mut display_screen = (*screen).clone();

            // Get cursor position before rendering (and potentially resizing)
            let (cursor_row, cursor_col) = display_screen.cursor_position();

            let widget = PtyWidget::new(&mut display_screen).dimmed(!is_active);
            frame.render_widget(widget, pane_area);

            // Position the cursor in the active pane
            if is_active {
                inner_area = pane_area;
                let cursor_x = pane_area.x + cursor_col;
                let cursor_y = pane_area.y + cursor_row;
                // Only set cursor if it's within the visible area
                if cursor_x < pane_area.x + pane_area.width && cursor_y < pane_area.y + pane_area.height {
                    frame.set_cursor_position((cursor_x, cursor_y));
                }
            }

            // Render divider after this pane (if not the last pane)
            if i < num_panes - 1 {
                let divider_area = chunks[i * 2 + 1];
                for y in divider_area.y..divider_area.y + divider_area.height {
                    frame.buffer_mut()[(divider_area.x, y)]
                        .set_char('│')
                        .set_style(divider_style);
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
