use std::path::PathBuf;

use crate::session::{AttachedSession, DetachedSession};

/// Which view is currently active in a session pair
#[derive(Clone, Copy, PartialEq, Default)]
pub enum SessionView {
    #[default]
    Claude,
    Shell,
}

/// Activity status of a Claude session (for hook notifications)
#[derive(Clone, Copy, PartialEq, Default)]
pub enum SessionActivity {
    #[default]
    Active,
    /// Claude stopped and needs user attention
    Stopped,
}

/// An active session pair - claude session is attached (can receive input)
/// Shell sessions are managed separately in TerminalMultiplexer
pub struct ActivePair {
    pub name: String,
    pub path: PathBuf,
    pub view: SessionView,
    pub claude: AttachedSession,
    /// Whether this session was started via resume (--continue flag)
    pub resumed: bool,
    /// Scroll offset for viewing scrollback history (0 = at bottom, showing current output)
    pub scroll_offset: usize,
    /// Activity status from hook notifications
    pub activity: SessionActivity,
}

impl ActivePair {
    pub fn new(name: String, path: PathBuf, claude: AttachedSession, resumed: bool) -> Self {
        Self {
            name,
            path,
            view: SessionView::Claude,
            claude,
            resumed,
            scroll_offset: 0,
            activity: SessionActivity::Active,
        }
    }

    pub fn detach(self) -> BackgroundPair {
        BackgroundPair {
            name: self.name,
            path: self.path,
            last_view: self.view,
            claude: self.claude.detach(),
            resumed: self.resumed,
            scroll_offset: self.scroll_offset,
            activity: self.activity,
        }
    }
}

/// A background session pair - claude session is detached
/// Shell sessions are managed separately in TerminalMultiplexer
pub struct BackgroundPair {
    pub name: String,
    pub path: PathBuf,
    pub last_view: SessionView,
    pub claude: DetachedSession,
    /// Whether this session was started via resume (--continue flag)
    pub resumed: bool,
    /// Scroll offset for viewing scrollback history (0 = at bottom, showing current output)
    pub scroll_offset: usize,
    /// Activity status from hook notifications
    pub activity: SessionActivity,
}

impl BackgroundPair {
    pub fn attach(self) -> anyhow::Result<ActivePair> {
        Ok(ActivePair {
            name: self.name,
            path: self.path,
            view: self.last_view,
            claude: self.claude.attach()?,
            resumed: self.resumed,
            scroll_offset: self.scroll_offset,
            // Preserve activity state - only cleared when user sends input
            activity: self.activity,
        })
    }
}
