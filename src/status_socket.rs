use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

/// Events received from Claude hooks via the status socket
#[derive(Debug, Clone)]
pub struct StatusEvent {
    pub session: String,
    pub event: EventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Stop,
    Notification,
}

/// Unix socket listener for receiving status events from Claude hooks
pub struct StatusSocket {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl StatusSocket {
    /// Create a new status socket at ~/.shepherd/status.sock
    pub fn new() -> std::io::Result<Self> {
        let socket_path = dirs::home_dir()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "No home directory"))?
            .join(".shepherd")
            .join("status.sock");

        // Ensure directory exists
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Remove existing socket file if it exists
        if socket_path.exists() {
            std::fs::remove_file(&socket_path)?;
        }

        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        Ok(Self {
            listener,
            socket_path,
        })
    }

    /// Get the socket path for passing to child processes
    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    /// Poll for incoming events (non-blocking)
    /// Returns a Vec of events received since last poll
    pub fn poll(&self) -> Vec<StatusEvent> {
        let mut events = Vec::new();

        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    // Read the JSON message from the client
                    let reader = BufReader::new(stream);
                    for line in reader.lines().map_while(Result::ok) {
                        if let Some(event) = Self::parse_event(&line) {
                            events.push(event);
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No more pending connections
                    break;
                }
                Err(_) => {
                    // Other errors - ignore and continue
                    break;
                }
            }
        }

        events
    }

    /// Parse a JSON event message
    fn parse_event(line: &str) -> Option<StatusEvent> {
        // Simple JSON parsing without serde
        // Expected format: {"session":"name","event":"stop"|"notification"}
        let line = line.trim();
        if !line.starts_with('{') || !line.ends_with('}') {
            return None;
        }

        let inner = &line[1..line.len() - 1];

        let mut session = None;
        let mut event = None;

        for part in inner.split(',') {
            let part = part.trim();
            if let Some((key, value)) = part.split_once(':') {
                let key = key.trim().trim_matches('"');
                let value = value.trim().trim_matches('"');

                match key {
                    "session" => session = Some(value.to_string()),
                    "event" => {
                        event = match value {
                            "stop" => Some(EventKind::Stop),
                            "notification" => Some(EventKind::Notification),
                            _ => None,
                        }
                    }
                    _ => {}
                }
            }
        }

        match (session, event) {
            (Some(session), Some(event)) => Some(StatusEvent { session, event }),
            _ => None,
        }
    }
}

impl Drop for StatusSocket {
    fn drop(&mut self) {
        // Clean up the socket file
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_event_stop() {
        let event = StatusSocket::parse_event(r#"{"session":"test-session","event":"stop"}"#);
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.session, "test-session");
        assert_eq!(event.event, EventKind::Stop);
    }

    #[test]
    fn test_parse_event_notification() {
        let event = StatusSocket::parse_event(r#"{"session":"my-feature","event":"notification"}"#);
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.session, "my-feature");
        assert_eq!(event.event, EventKind::Notification);
    }

    #[test]
    fn test_parse_event_invalid() {
        assert!(StatusSocket::parse_event("not json").is_none());
        assert!(StatusSocket::parse_event(r#"{"session":"test"}"#).is_none());
        assert!(StatusSocket::parse_event(r#"{"event":"stop"}"#).is_none());
    }
}
