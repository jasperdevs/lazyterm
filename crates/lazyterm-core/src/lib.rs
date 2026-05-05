use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for SessionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SessionId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<SessionId> for String {
    fn from(value: SessionId) -> Self {
        value.0
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AgentKind {
    Shell,
    Codex,
    Claude,
    OpenCode,
    Gemini,
    Aider,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Shell => "Shell",
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::OpenCode => "OpenCode",
            Self::Gemini => "Gemini",
            Self::Aider => "Aider",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SessionStatus {
    Running,
    Waiting,
    NeedsInput,
    Failed,
    Done,
}

impl SessionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::NeedsInput => "needs input",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRef {
    pub cwd: PathBuf,
    pub git_branch: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub title: String,
    pub agent: AgentKind,
    pub status: SessionStatus,
    pub workspace: WorkspaceRef,
    pub command: String,
    pub last_activity: String,
    pub notification: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_serializes_as_a_plain_string() {
        let id = SessionId::new("session-123");

        assert_eq!(
            serde_json::to_string(&id).expect("serialize session id"),
            "\"session-123\""
        );
        assert_eq!(
            serde_json::from_str::<SessionId>("\"session-123\"").expect("deserialize session id"),
            id
        );
    }

    #[test]
    fn session_id_conversions_are_straightforward() {
        let id: SessionId = "session-456".into();

        assert_eq!(id.as_ref(), "session-456");
        assert_eq!(String::from(id), "session-456");
    }
}
