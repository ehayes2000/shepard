use std::io;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use crossterm::cursor::MoveTo;
use crossterm::terminal::{Clear, ClearType};
use crossterm::ExecutableCommand;

use crate::session::Session;

#[derive(PartialEq)]
pub enum InputMode {
    Normal,
    CreatingSession,
}

pub struct App {
    pub sessions: Vec<Session>,
    pub active_session: Option<usize>,
    pub selected_index: usize,
    pub show_overlay: bool,
    pub input_mode: InputMode,
    pub input_buffer: String,
    pub status_message: String,
    pub should_quit: bool,
    pub terminal_size: (u16, u16),
    pub needs_full_render: bool,
    repo_path: PathBuf,
    repo_name: String,
    worktrees_dir: PathBuf,
}

impl App {
    pub fn new(terminal_size: (u16, u16)) -> Result<Self> {
        let repo_path = std::env::current_dir().context("Failed to get current directory")?;

        let repo_name = repo_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("repo")
            .to_string();

        let worktrees_dir = std::env::var("WORKTREES")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join("worktrees"));

        if !worktrees_dir.exists() {
            std::fs::create_dir_all(&worktrees_dir)?;
        }

        Ok(Self {
            sessions: Vec::new(),
            active_session: None,
            selected_index: 0,
            show_overlay: true,
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            repo_path,
            repo_name,
            worktrees_dir,
            status_message: String::from(
                "n: new | Enter: switch | d: delete | Cmd+b: toggle | q: quit",
            ),
            should_quit: false,
            terminal_size,
            needs_full_render: false,
        })
    }

    pub fn create_session(&mut self, name: String) -> Result<()> {
        let branch = &name;
        let worktree_name = format!("{}-{}", self.repo_name, name);
        let worktree_path = self.worktrees_dir.join(&worktree_name);

        self.status_message = format!("Creating worktree '{}'...", worktree_name);

        let output = Command::new("git")
            .args(["worktree", "add", "-b", branch, worktree_path.to_str().unwrap()])
            .current_dir(&self.repo_path)
            .output();

        let worktree_ok = match output {
            Ok(out) if out.status.success() => true,
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if stderr.contains("already exists") {
                    let output2 = Command::new("git")
                        .args(["worktree", "add", worktree_path.to_str().unwrap(), branch])
                        .current_dir(&self.repo_path)
                        .output();
                    matches!(output2, Ok(out2) if out2.status.success()) || worktree_path.exists()
                } else {
                    self.status_message = format!("Failed: {}", stderr.trim());
                    return Ok(());
                }
            }
            Err(e) => {
                self.status_message = format!("Failed to run git: {}", e);
                return Ok(());
            }
        };

        if !worktree_ok && !worktree_path.exists() {
            self.status_message = "Failed to create worktree".to_string();
            return Ok(());
        }

        let session = Session::new(name.clone(), worktree_path, self.terminal_size)?;

        self.sessions.push(session);
        let idx = self.sessions.len() - 1;
        self.active_session = Some(idx);
        self.selected_index = idx;
        self.show_overlay = false;
        self.needs_full_render = true;
        self.status_message = format!("Session '{}' created", name);

        Ok(())
    }

    pub fn switch_to_session(&mut self, index: usize, stdout: &mut io::Stdout) -> Result<()> {
        if index < self.sessions.len() {
            self.active_session = Some(index);
            self.show_overlay = false;

            // Drain any pending output before rendering
            self.sessions[index].drain_output();

            // Render the session's terminal screen state (it handles clearing each line)
            self.sessions[index].render_screen(stdout)?;

            // Mark that we need a full render on next frame to catch any additional output
            self.needs_full_render = true;
        }
        Ok(())
    }

    pub fn delete_session(&mut self, index: usize) {
        if index >= self.sessions.len() {
            return;
        }

        let session = &self.sessions[index];
        let name = session.name.clone();
        let worktree_path = session.worktree_path.clone();

        let _ = Command::new("git")
            .args(["worktree", "remove", "--force", worktree_path.to_str().unwrap()])
            .current_dir(&self.repo_path)
            .output();

        self.sessions.remove(index);

        if self.sessions.is_empty() {
            self.active_session = None;
            self.selected_index = 0;
        } else {
            self.selected_index = self.selected_index.min(self.sessions.len() - 1);
            if self.active_session == Some(index) {
                self.active_session = Some(self.selected_index);
            } else if let Some(active) = self.active_session {
                if active > index {
                    self.active_session = Some(active - 1);
                }
            }
        }

        self.status_message = format!("Session '{}' deleted", name);
    }

    pub fn select_next(&mut self) {
        if !self.sessions.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.sessions.len();
        }
    }

    pub fn select_previous(&mut self) {
        if !self.sessions.is_empty() {
            self.selected_index = if self.selected_index == 0 {
                self.sessions.len() - 1
            } else {
                self.selected_index - 1
            };
        }
    }
}
