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
    SendText {
        id: Option<String>,
        text: String,
        enter: bool,
    },
    CloseOtherSessions,
    FocusAttention,
    SetLayout {
        layout: TileLayout,
    },
    SetDensity {
        density: TerminalDensity,
    },
    AgentHealth,
    Status,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ApiResponse {
    Ack,
    Sessions(Vec<SessionSummary>),
    AgentHealth(Vec<AgentHealthSummary>),
    Error { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentHealthSummary {
    pub agent: AgentKind,
    pub command: String,
    pub available: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TileLayout {
    Grid,
    Columns,
    Rows,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TerminalDensity {
    Compact,
    Default,
    Roomy,
}
