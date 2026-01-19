use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use std::collections::HashSet;
use std::path::PathBuf;

/// A dialog for selecting and deleting worktrees.
pub struct WorktreeCleanupDialog {
    /// List of worktree paths
    worktrees: Vec<PathBuf>,
    /// Selection state for the list
    state: ListState,
    /// Set of selected indices (multi-select)
    selected: HashSet<usize>,
    /// Current filter query
    query: String,
    /// Filtered indices matching the query
    filtered_indices: Vec<usize>,
    /// Paths that have active sessions
    active_paths: HashSet<PathBuf>,
}

impl WorktreeCleanupDialog {
    pub fn new() -> Self {
        let mut state = ListState::default();
        state.select(Some(0));
        Self {
            worktrees: Vec::new(),
            state,
            selected: HashSet::new(),
            query: String::new(),
            filtered_indices: Vec::new(),
            active_paths: HashSet::new(),
        }
    }

    /// Reset the dialog state for a fresh view.
    pub fn reset(&mut self) {
        self.worktrees.clear();
        self.selected.clear();
        self.query.clear();
        self.filtered_indices.clear();
        self.active_paths.clear();
        self.state.select(Some(0));
    }

    /// Set the list of worktrees to display with active session info.
    pub fn set_worktrees_with_active(
        &mut self,
        worktrees: Vec<PathBuf>,
        active_paths: HashSet<PathBuf>,
    ) {
        self.worktrees = worktrees;
        self.active_paths = active_paths;
        self.selected.clear();
        self.update_filter();
    }

    /// Add a character to the filter query.
    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
    }

    /// Remove the last character from the filter query.
    pub fn pop_char(&mut self) {
        self.query.pop();
    }

    /// Update the filtered indices based on the current query.
    pub fn update_filter(&mut self) {
        let query_lower = self.query.to_lowercase();

        self.filtered_indices = self
            .worktrees
            .iter()
            .enumerate()
            .filter(|(_, path)| {
                if query_lower.is_empty() {
                    true
                } else {
                    path.to_string_lossy().to_lowercase().contains(&query_lower)
                }
            })
            .map(|(i, _)| i)
            .collect();

        // Ensure selection stays valid
        if self.filtered_indices.is_empty() {
            self.state.select(None);
        } else {
            let current = self.state.selected().unwrap_or(0);
            if current >= self.filtered_indices.len() {
                self.state.select(Some(self.filtered_indices.len() - 1));
            }
        }
    }

    /// Move selection up in the filtered list.
    pub fn move_up(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.state.selected().unwrap_or(0);
        let next = if current == 0 {
            self.filtered_indices.len() - 1
        } else {
            current - 1
        };
        self.state.select(Some(next));
    }

    /// Move selection down in the filtered list.
    pub fn move_down(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.state.selected().unwrap_or(0);
        let next = if current >= self.filtered_indices.len() - 1 {
            0
        } else {
            current + 1
        };
        self.state.select(Some(next));
    }

    /// Toggle selection on the currently highlighted item.
    pub fn toggle_selection(&mut self) {
        if let Some(filtered_idx) = self.state.selected()
            && let Some(&original_idx) = self.filtered_indices.get(filtered_idx)
        {
            if self.selected.contains(&original_idx) {
                self.selected.remove(&original_idx);
            } else {
                self.selected.insert(original_idx);
            }
        }
    }

    /// Check if any worktrees are selected.
    pub fn has_selections(&self) -> bool {
        !self.selected.is_empty()
    }

    /// Get the selected worktree paths.
    pub fn get_selected_worktrees(&self) -> Vec<PathBuf> {
        self.selected
            .iter()
            .filter_map(|&idx| self.worktrees.get(idx).cloned())
            .collect()
    }

    /// Get the currently highlighted worktree (if any).
    pub fn get_current_worktree(&self) -> Option<PathBuf> {
        let filtered_idx = self.state.selected()?;
        let original_idx = self.filtered_indices.get(filtered_idx)?;
        self.worktrees.get(*original_idx).cloned()
    }

    /// Check if there are any worktrees to display.
    pub fn is_empty(&self) -> bool {
        self.worktrees.is_empty()
    }

    /// Render the worktree cleanup dialog.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Calculate popup dimensions
        let max_path_len = self
            .worktrees
            .iter()
            .map(|p| p.to_string_lossy().len())
            .max()
            .unwrap_or(20);

        // Width: checkbox (4) + path + padding + borders
        let content_width = 4 + max_path_len + 4;
        let popup_width = content_width.max(50).min(area.width as usize - 4) as u16;

        // Height: warning (2) + input (3) + list items + footer (2) + borders
        let max_visible = 10usize;
        let list_height = self.filtered_indices.len().min(max_visible).max(1) as u16;
        let popup_height = (2 + 3 + list_height + 2 + 2).min(area.height - 2);

        // Center the popup
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        // Clear the popup area
        frame.render_widget(Clear, popup_area);

        // Main block with title
        let block = Block::default()
            .title(" Worktree Cleanup ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::White))
            .style(Style::default().bg(Color::Black));
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        // Warning area (2 lines)
        let warning_area = Rect::new(inner.x, inner.y, inner.width, 2);
        let warning = Paragraph::new(Line::from(vec![
            Span::styled(
                "WARNING: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Deletion is permanent and cannot be undone",
                Style::default().fg(Color::Red),
            ),
        ]));
        frame.render_widget(warning, warning_area);

        // Input area (3 lines)
        let input_area = Rect::new(inner.x, inner.y + 2, inner.width, 3);
        let input_text = format!("{}_", self.query);
        let input = Paragraph::new(input_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Gray))
                    .title(" Filter "),
            )
            .style(Style::default().fg(Color::White));
        frame.render_widget(input, input_area);

        // List area
        let list_area_height = inner.height.saturating_sub(2 + 3 + 2);
        let list_area = Rect::new(inner.x, inner.y + 5, inner.width, list_area_height);

        if self.worktrees.is_empty() {
            let empty_msg =
                Paragraph::new("No worktrees found").style(Style::default().fg(Color::DarkGray));
            frame.render_widget(empty_msg, list_area);
        } else {
            // Build list items with checkboxes
            let items: Vec<ListItem> = self
                .filtered_indices
                .iter()
                .map(|&i| {
                    let path = &self.worktrees[i];
                    let is_selected = self.selected.contains(&i);
                    let is_active = self.active_paths.contains(path);
                    let active_marker = if is_active { " [ACTIVE]" } else { "" };
                    let available_width =
                        (popup_width as usize).saturating_sub(8 + active_marker.len()); // borders + checkbox + marker

                    let path_str = path.to_string_lossy();
                    let path_display = if path_str.len() > available_width {
                        format!(
                            "...{}",
                            &path_str[path_str.len().saturating_sub(available_width - 3)..]
                        )
                    } else {
                        path_str.to_string()
                    };

                    let checkbox = if is_selected {
                        Span::styled("[x] ", Style::default().fg(Color::Green))
                    } else {
                        Span::styled("[ ] ", Style::default().fg(Color::Gray))
                    };

                    let mut spans = vec![
                        checkbox,
                        Span::styled(path_display, Style::default().fg(Color::White)),
                    ];
                    if is_active {
                        spans.push(Span::styled(
                            " [ACTIVE]",
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ));
                    }

                    Line::from(spans)
                })
                .map(ListItem::new)
                .collect();

            let list = List::new(items)
                .highlight_style(
                    Style::default()
                        .bg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("> ");

            frame.render_stateful_widget(list, list_area, &mut self.state);
        }

        // Footer with controls
        let footer_area = Rect::new(inner.x, inner.y + inner.height - 2, inner.width, 2);
        let footer = Paragraph::new(Line::from(vec![
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": select  "),
            Span::styled(
                "d",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": delete  "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": close"),
        ]));
        frame.render_widget(footer, footer_area);
    }
}

impl Default for WorktreeCleanupDialog {
    fn default() -> Self {
        Self::new()
    }
}
