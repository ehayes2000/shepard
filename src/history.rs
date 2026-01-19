use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

const MAX_RECENT_PER_WORKSPACE: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentSession {
    pub name: String,
    pub path: PathBuf,
}

/// Stores recent sessions per startup directory (workspace).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionHistory {
    recent_sessions: HashMap<PathBuf, VecDeque<RecentSession>>,
}

impl SessionHistory {
    fn history_path() -> anyhow::Result<PathBuf> {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not find home directory"))?;
        Ok(home.join(".shepard").join("history.json"))
    }

    pub fn load() -> anyhow::Result<Self> {
        let path = Self::history_path()?;

        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let history: SessionHistory = serde_json::from_str(&contents)?;
            Ok(history)
        } else {
            Ok(SessionHistory::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::history_path()?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, contents)?;
        Ok(())
    }

    pub fn set_recent_session(
        &mut self,
        startup_path: PathBuf,
        session_name: String,
        session_path: PathBuf,
    ) -> anyhow::Result<()> {
        let entry = RecentSession {
            name: session_name,
            path: session_path,
        };

        let sessions = self.recent_sessions.entry(startup_path).or_default();

        // Remove existing entry if present (will be re-added at front)
        sessions.retain(|s| s != &entry);

        // Add to front
        sessions.push_front(entry);

        // Trim to max size
        while sessions.len() > MAX_RECENT_PER_WORKSPACE {
            sessions.pop_back();
        }

        self.save()
    }

    /// Get the most recent session for a workspace
    pub fn get_recent_session(&self, startup_path: &PathBuf) -> Option<&RecentSession> {
        self.recent_sessions
            .get(startup_path)
            .and_then(|sessions| sessions.front())
    }

    /// Get all recent sessions for a workspace (most recent first)
    pub fn get_recent_sessions(&self, startup_path: &PathBuf) -> impl Iterator<Item = &RecentSession> {
        self.recent_sessions
            .get(startup_path)
            .into_iter()
            .flat_map(|sessions| sessions.iter())
    }
}
