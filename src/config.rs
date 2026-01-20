use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub claude_args: Vec<String>,
    pub workflows_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let workflows_path = dirs::home_dir()
            .map(|h| h.join("worktrees"))
            .unwrap_or_else(|| PathBuf::from("~/worktrees"));

        Self {
            claude_args: vec!["--dangerously-skip-permissions".to_string()],
            workflows_path,
        }
    }
}

impl Config {
    fn config_path() -> anyhow::Result<PathBuf> {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not find home directory"))?;
        Ok(home.join(".shepherd").join("config.json"))
    }

    pub fn load() -> anyhow::Result<Self> {
        let path = Self::config_path()?;

        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let config: Config = serde_json::from_str(&contents)?;
            Ok(config)
        } else {
            let config = Config::default();
            config.save()?;
            Ok(config)
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::config_path()?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, contents)?;
        Ok(())
    }
}
