use crate::config::Config;
use std::process::Command;

use super::{SessionMetadata, Workflow};

/// Workflow that creates git worktrees for each session
pub struct WorktreeWorkflow;

impl WorktreeWorkflow {
    /// Get the repository name from the current directory
    fn get_repo_name() -> anyhow::Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()?;

        if !output.status.success() {
            anyhow::bail!("not in a git repository");
        }

        let repo_path = String::from_utf8(output.stdout)?.trim().to_string();
        let repo_name = std::path::Path::new(&repo_path)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("could not determine repository name"))?
            .to_string();

        Ok(repo_name)
    }

    /// Get the main branch name (main or master)
    fn get_main_branch() -> anyhow::Result<String> {
        // Check if 'main' exists
        let output = Command::new("git")
            .args(["rev-parse", "--verify", "main"])
            .output()?;

        if output.status.success() {
            return Ok("main".to_string());
        }

        // Fall back to 'master'
        let output = Command::new("git")
            .args(["rev-parse", "--verify", "master"])
            .output()?;

        if output.status.success() {
            return Ok("master".to_string());
        }

        anyhow::bail!("could not find main or master branch")
    }
}

impl Workflow for WorktreeWorkflow {
    fn pre_session_hook(&self, session_name: &str, config: &Config, _startup_path: &std::path::Path) -> anyhow::Result<SessionMetadata> {
        let repo_name = Self::get_repo_name()?;
        let main_branch = Self::get_main_branch()?;

        // Build worktree path: <workflows_path>/<reponame>/<sessionname>
        let worktree_path = config.workflows_path.join(&repo_name).join(session_name);

        // Fetch latest from origin
        let output = Command::new("git")
            .args(["fetch", "origin", &main_branch])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("failed to fetch origin/{}: {}", main_branch, stderr);
        }

        // Create the worktree with a new branch based on origin/main
        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                session_name,
                worktree_path.to_str().unwrap(),
                &format!("origin/{}", main_branch),
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("failed to create worktree: {}", stderr);
        }

        Ok(SessionMetadata {
            path: worktree_path,
        })
    }
}
