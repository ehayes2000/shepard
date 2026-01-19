use std::path::PathBuf;

use crate::session::{AttachedSession, DetachedSession};

/// Which view is currently active in a session pair
#[derive(Clone, Copy, PartialEq, Default)]
pub enum SessionView {
    #[default]
    Claude,
    Shell,
}

/// An active session pair - both claude and shell are attached (can receive input)
pub struct ActivePair {
    pub name: String,
    pub path: PathBuf,
    pub view: SessionView,
    pub claude: AttachedSession,
    pub shell: Option<AttachedSession>,
    /// Whether this session was started via resume (--continue flag)
    pub resumed: bool,
}

impl ActivePair {
    pub fn new(
        name: String,
        path: PathBuf,
        claude: AttachedSession,
        resumed: bool,
    ) -> Self {
        Self {
            name,
            path,
            view: SessionView::Claude,
            claude,
            shell: None,
            resumed,
        }
    }

    pub fn detach(self) -> BackgroundPair {
        BackgroundPair {
            name: self.name,
            path: self.path,
            last_view: self.view,
            claude: self.claude.detach(),
            shell: self.shell.map(|s| s.detach()),
            resumed: self.resumed,
        }
    }
}

/// A background session pair - both sessions are detached
pub struct BackgroundPair {
    pub name: String,
    pub path: PathBuf,
    pub last_view: SessionView,
    pub claude: DetachedSession,
    pub shell: Option<DetachedSession>,
    /// Whether this session was started via resume (--continue flag)
    pub resumed: bool,
}

impl BackgroundPair {
    pub fn attach(self) -> anyhow::Result<ActivePair> {
        Ok(ActivePair {
            name: self.name,
            path: self.path,
            view: self.last_view,
            claude: self.claude.attach()?,
            shell: self.shell.map(|s| s.attach()).transpose()?,
            resumed: self.resumed,
        })
    }
}
