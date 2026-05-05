use lazyterm_core::{AgentKind, SessionStatus};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentPreset {
    pub kind: AgentKind,
    pub command: &'static str,
    pub args: &'static [&'static str],
}

pub const AGENT_PRESETS: &[AgentPreset] = &[
    AgentPreset {
        kind: AgentKind::Shell,
        command: "",
        args: &[],
    },
    AgentPreset {
        kind: AgentKind::Codex,
        command: "codex",
        args: &[],
    },
    AgentPreset {
        kind: AgentKind::Claude,
        command: "claude",
        args: &[],
    },
    AgentPreset {
        kind: AgentKind::OpenCode,
        command: "opencode",
        args: &[],
    },
    AgentPreset {
        kind: AgentKind::Gemini,
        command: "gemini",
        args: &[],
    },
    AgentPreset {
        kind: AgentKind::Aider,
        command: "aider",
        args: &[],
    },
];

pub fn detect_status(output: &str) -> SessionStatus {
    let text = normalize_output(output);

    if is_failed(&text) {
        SessionStatus::Failed
    } else if is_needs_input(&text) {
        SessionStatus::NeedsInput
    } else if is_done(&text) {
        SessionStatus::Done
    } else if is_waiting(&text) {
        SessionStatus::Waiting
    } else {
        SessionStatus::Running
    }
}

fn normalize_output(output: &str) -> String {
    output
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect()
}

fn contains_word(text: &str, needle: &str) -> bool {
    text.split_whitespace().any(|word| word == needle)
}

fn contains_any_word(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| contains_word(text, needle))
}

fn contains_any_phrase(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn is_failed(text: &str) -> bool {
    contains_any_phrase(
        text,
        &[
            "permission denied",
            "access denied",
            "operation not permitted",
            "command not found",
            "not found",
            "timed out",
        ],
    ) || contains_any_word(
        text,
        &[
            "failed",
            "failure",
            "fatal",
            "error",
            "exception",
            "panic",
            "aborted",
        ],
    )
}

fn is_needs_input(text: &str) -> bool {
    contains_any_phrase(
        text,
        &[
            "needs input",
            "input required",
            "approval required",
            "permission required",
            "requires permission",
            "waiting for approval",
            "awaiting approval",
        ],
    ) || contains_any_word(
        text,
        &[
            "approve",
            "approval",
            "confirm",
            "continue",
            "proceed",
            "authorize",
        ],
    )
}

fn is_done(text: &str) -> bool {
    if text.contains("not complete") || text.contains("not done") || text.contains("incomplete") {
        return false;
    }

    contains_any_phrase(text, &["completed successfully", "all done"])
        || contains_any_word(
            text,
            &[
                "done",
                "finished",
                "complete",
                "completed",
                "success",
                "successful",
                "successfully",
                "succeeded",
            ],
        )
}

fn is_waiting(text: &str) -> bool {
    contains_any_phrase(text, &["waiting for", "in progress"])
        || contains_any_word(text, &["waiting", "queued", "pending", "blocked"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_permission_waits() {
        assert_eq!(
            detect_status("Approve this command to continue?"),
            SessionStatus::NeedsInput
        );
    }

    #[test]
    fn detects_waiting_transcript_lines() {
        assert_eq!(
            detect_status("Waiting for approval before proceeding."),
            SessionStatus::NeedsInput
        );
        assert_eq!(
            detect_status("waiting on tool response"),
            SessionStatus::Waiting
        );
    }

    #[test]
    fn detects_failures_without_confusing_permissions() {
        assert_eq!(
            detect_status("Permission denied while launching codex."),
            SessionStatus::Failed
        );
        assert_eq!(
            detect_status("error: command exited with status 1"),
            SessionStatus::Failed
        );
    }

    #[test]
    fn detects_done_without_matching_incomplete() {
        assert_eq!(
            detect_status("Task completed successfully."),
            SessionStatus::Done
        );
        assert_eq!(detect_status("incomplete setup"), SessionStatus::Running);
    }
}
