use std::path::PathBuf;

use lazyterm_api::{
    AgentHealthSummary, AgentKind, ApiRequest, ApiResponse, SessionId, SessionStatus,
    SessionSummary, TerminalDensity, TileLayout, WorkspaceRef,
};
use serde_json::json;

#[test]
fn new_session_request_json_is_stable() {
    let request = ApiRequest::NewSession {
        cwd: PathBuf::from("repo"),
        agent: AgentKind::Codex,
        task: Some("fix parser".into()),
    };

    let value = serde_json::to_value(&request).expect("serialize request");

    assert_eq!(
        value,
        json!({
            "NewSession": {
                "cwd": "repo",
                "agent": "Codex",
                "task": "fix parser"
            }
        })
    );
    assert_eq!(
        serde_json::from_value::<ApiRequest>(value).expect("deserialize request"),
        request
    );
}

#[test]
fn sessions_response_preserves_summary_contract() {
    let summary = SessionSummary {
        id: SessionId::new("shell-7"),
        title: "build".into(),
        agent: AgentKind::OpenCode,
        status: SessionStatus::NeedsInput,
        workspace: WorkspaceRef {
            cwd: PathBuf::from("repo"),
            git_branch: Some("main".into()),
        },
        command: "opencode".into(),
        last_activity: "waiting".into(),
        notification: Some("approve?".into()),
    };
    let response = ApiResponse::Sessions(vec![summary.clone()]);

    let value = serde_json::to_value(&response).expect("serialize response");

    assert_eq!(value["Sessions"][0]["id"], "shell-7");
    assert_eq!(value["Sessions"][0]["workspace"]["git_branch"], "main");
    assert_eq!(
        serde_json::from_value::<ApiResponse>(value).expect("deserialize response"),
        response
    );
}

#[test]
fn control_enums_round_trip_as_named_variants() {
    assert_eq!(
        serde_json::to_value(TileLayout::Columns).expect("serialize layout"),
        json!("Columns")
    );
    assert_eq!(
        serde_json::to_value(TerminalDensity::Roomy).expect("serialize density"),
        json!("Roomy")
    );
    assert_eq!(
        serde_json::to_value(AgentHealthSummary {
            agent: AgentKind::Claude,
            command: "claude".into(),
            available: true,
        })
        .expect("serialize health"),
        json!({
            "agent": "Claude",
            "command": "claude",
            "available": true
        })
    );
}
