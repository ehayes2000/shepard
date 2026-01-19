use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;

const MAX_HISTORY_SIZE: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionHistory {
    entries: VecDeque<HistoryEntry>,
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

    /// Add or update an entry in history. If an entry with the same name+path exists,
    /// it's moved to the front. Otherwise a new entry is added at the front.
    pub fn touch(&mut self, name: String, path: PathBuf) {
        let entry = HistoryEntry { name, path };

        // Remove existing entry if present
        self.entries.retain(|e| e != &entry);

        // Add to front
        self.entries.push_front(entry);

        // Trim to max size
        while self.entries.len() > MAX_HISTORY_SIZE {
            self.entries.pop_back();
        }
    }

    /// Get all history entries (most recent first)
    pub fn entries(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter()
    }

    /// Get a history entry by name
    pub fn get_by_name(&self, name: &str) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.name == name)
    }
}
