use std::path::PathBuf;

pub use lazyterm_core::{AgentKind, SessionId, SessionStatus, SessionSummary, WorkspaceRef};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ApiRequest {
    NewSession {
        cwd: PathBuf,
        agent: AgentKind,
        task: Option<String>,
    },
    ListSessions,
    FocusSession {
        id: String,
    },
    Status,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ApiResponse {
    Ack,
    Sessions(Vec<SessionSummary>),
    Error { message: String },
}
