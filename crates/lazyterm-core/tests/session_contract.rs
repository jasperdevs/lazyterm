use std::path::PathBuf;

use lazyterm_core::{AgentKind, SessionId, SessionStatus, SessionSummary, WorkspaceRef};
use serde_json::json;

#[test]
fn session_summary_json_is_stable_across_crates() {
    let summary = SessionSummary {
        id: SessionId::new("shell-2"),
        title: "watch".into(),
        agent: AgentKind::Gemini,
        status: SessionStatus::Running,
        workspace: WorkspaceRef {
            cwd: PathBuf::from("repo"),
            git_branch: None,
        },
        command: "gemini".into(),
        last_activity: "output".into(),
        notification: None,
    };

    let value = serde_json::to_value(&summary).expect("serialize summary");

    assert_eq!(
        value,
        json!({
            "id": "shell-2",
            "title": "watch",
            "agent": "Gemini",
            "status": "Running",
            "workspace": {
                "cwd": "repo",
                "git_branch": null
            },
            "command": "gemini",
            "last_activity": "output",
            "notification": null
        })
    );
    assert_eq!(
        serde_json::from_value::<SessionSummary>(value).expect("deserialize summary"),
        summary
    );
}

#[test]
fn user_facing_labels_are_stable() {
    assert_eq!(AgentKind::OpenCode.label(), "OpenCode");
    assert_eq!(SessionStatus::NeedsInput.label(), "needs input");
}
