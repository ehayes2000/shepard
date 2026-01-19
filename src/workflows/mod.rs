mod worktree;

pub use worktree::WorktreeWorkflow;

use crate::config::Config;
use crate::session_manager::StatusMessage;
use std::path::{Path, PathBuf};

/// Metadata returned by a workflow's pre-session hook
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub path: PathBuf,
}

/// A workflow defines how sessions are created and configured
pub trait Workflow: Send + Sync {
    /// Name of this workflow for error messages
    fn name(&self) -> &'static str;

    /// Called before a session is created. Returns metadata for the session.
    fn pre_session_hook(
        &self,
        session_name: &str,
        config: &Config,
        startup_path: &Path,
    ) -> Result<SessionMetadata, StatusMessage>;
}
