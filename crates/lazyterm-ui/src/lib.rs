use gpui::{div, prelude::*, px, rgb, Context, IntoElement, Render, SharedString, Window};
use lazyterm_agents::AGENT_PRESETS;
use std::path::PathBuf;

use lazyterm_core::{AgentKind, SessionId, SessionStatus, SessionSummary, WorkspaceRef};

const APP_BG: u32 = 0x0b0d11;
const SIDEBAR_BG: u32 = 0x11161d;
const SHELL_PANEL_BG: u32 = 0x151a22;
const SURFACE_BG: u32 = 0x0f131a;
const CARD_BG: u32 = 0x131a24;
const CARD_BG_ACTIVE: u32 = 0x18212d;
const BORDER: u32 = 0x232b39;
const BORDER_ACTIVE: u32 = 0x3a5170;
const TEXT_PRIMARY: u32 = 0xf4f7fb;
const TEXT_SECONDARY: u32 = 0xc7cfdb;
const TEXT_MUTED: u32 = 0x8d97a8;
const TEXT_SUBTLE: u32 = 0x6f7a8d;
const CHIP_BG: u32 = 0x1d2532;
const CHIP_BG_ACTIVE: u32 = 0x253345;

pub struct LazytermApp {
    sessions: Vec<SessionSummary>,
}

impl LazytermApp {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            sessions: demo_sessions(),
        }
    }

    fn active_session_index(&self) -> usize {
        self.sessions
            .iter()
            .position(|session| matches!(session.status, SessionStatus::NeedsInput))
            .unwrap_or(0)
    }

    fn render_sidebar(&self) -> impl IntoElement {
        let active_index = self.active_session_index();

        let mut tabs = div().flex().flex_col().gap_2();
        for (index, session) in self.sessions.iter().enumerate() {
            tabs = tabs.child(self.render_session_tab(session, index == active_index));
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .w(px(320.0))
            .h_full()
            .p_4()
            .bg(rgb(SIDEBAR_BG))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .text_size(px(18.0))
                                    .child("Sessions"),
                            )
                            .child(
                                div()
                                    .text_color(rgb(TEXT_MUTED))
                                    .text_size(px(12.0))
                                    .child(format!("{} workspaces", self.sessions.len())),
                            ),
                    )
                    .child(self.render_pill(
                        SharedString::from(self.sessions.len().to_string()),
                        TEXT_PRIMARY,
                        CHIP_BG,
                    )),
            )
            .child(tabs)
            .child(
                div()
                    .mt_auto()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SHELL_PANEL_BG))
                    .p_3()
                    .child(
                        div()
                            .text_color(rgb(TEXT_SUBTLE))
                            .text_size(px(12.0))
                            .child(format!("{} presets loaded", AGENT_PRESETS.len())),
                    ),
            )
    }

    fn render_session_tab(&self, session: &SessionSummary, active: bool) -> impl IntoElement {
        let background = if active { CARD_BG_ACTIVE } else { CARD_BG };
        let border = if active { BORDER_ACTIVE } else { BORDER };
        let title_color = if active { TEXT_PRIMARY } else { TEXT_SECONDARY };
        let subtitle_color = if active { TEXT_SECONDARY } else { TEXT_SUBTLE };
        let accent = match session.status {
            SessionStatus::NeedsInput => 0x62a6ff,
            SessionStatus::Failed => 0xff667a,
            SessionStatus::Done => 0x76d58a,
            SessionStatus::Waiting => 0xe5b85c,
            SessionStatus::Running => 0x9aa6b2,
        };
        let branch = session
            .workspace
            .git_branch
            .as_deref()
            .unwrap_or("no branch");

        div()
            .flex()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(background))
            .p_3()
            .child(div().w(px(4.0)).rounded_full().bg(rgb(accent)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_color(rgb(title_color))
                                    .text_size(px(13.0))
                                    .child(SharedString::from(session.title.clone())),
                            )
                            .child(self.render_pill(
                                SharedString::from(session.status.label()),
                                accent,
                                if active { CHIP_BG_ACTIVE } else { CHIP_BG },
                            )),
                    )
                    .child(
                        div()
                            .text_color(rgb(subtitle_color))
                            .text_size(px(12.0))
                            .child(format!("{} · {}", branch, session.last_activity)),
                    ),
            )
    }

    fn render_terminal_panel(&self) -> impl IntoElement {
        let active = &self.sessions[self.active_session_index()];
        let branch = active
            .workspace
            .git_branch
            .as_deref()
            .unwrap_or("no branch");

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .p_5()
            .gap_4()
            .bg(rgb(APP_BG))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .text_size(px(20.0))
                                    .child("Terminal"),
                            )
                            .child(
                                div()
                                    .text_color(rgb(TEXT_MUTED))
                                    .text_size(px(13.0))
                                    .child(format!("{} · {}", active.title, branch)),
                            ),
                    )
                    .child(self.render_pill(
                        SharedString::from(active.agent.label()),
                        TEXT_SECONDARY,
                        CHIP_BG,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE_BG))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .px_4()
                            .py_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .text_size(px(13.0))
                                            .child("Live shell"),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(TEXT_SUBTLE))
                                            .text_size(px(12.0))
                                            .child(format!(
                                                "{} · {}",
                                                active.agent.label(),
                                                active.command
                                            )),
                                    ),
                            )
                            .child(self.render_pill(
                                SharedString::from(active.status.label()),
                                match active.status {
                                    SessionStatus::NeedsInput => 0x62a6ff,
                                    SessionStatus::Failed => 0xff667a,
                                    SessionStatus::Done => 0x76d58a,
                                    SessionStatus::Waiting => 0xe5b85c,
                                    SessionStatus::Running => 0x9aa6b2,
                                },
                                CHIP_BG,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .p_5()
                            .gap_2()
                            .text_color(rgb(TEXT_SECONDARY))
                            .text_size(px(13.0))
                            .child(format!("workspace: {}", active.workspace.cwd.display()))
                            .child(format!("branch: {}", branch))
                            .child(format!("command: {}", active.command))
                            .child("No live output yet."),
                    ),
            )
    }

    fn render_pill(&self, label: SharedString, fg: u32, bg: u32) -> impl IntoElement {
        div()
            .rounded_full()
            .px_2()
            .py_1()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .text_size(px(11.0))
            .child(label)
    }
}

fn demo_sessions() -> Vec<SessionSummary> {
    vec![
        SessionSummary {
            id: SessionId::new("codex-main"),
            title: "Lazyterm foundation".into(),
            agent: AgentKind::Codex,
            status: SessionStatus::NeedsInput,
            workspace: WorkspaceRef {
                cwd: PathBuf::from("C:/Users/bunny/Downloads/lazyterm"),
                git_branch: Some("main".into()),
            },
            command: "codex".into(),
            last_activity: "waiting for review".into(),
            notification: Some("needs review".into()),
        },
        SessionSummary {
            id: SessionId::new("shell-build"),
            title: "Build monitor".into(),
            agent: AgentKind::Shell,
            status: SessionStatus::Running,
            workspace: WorkspaceRef {
                cwd: PathBuf::from("C:/Users/bunny/Downloads/lazyterm"),
                git_branch: Some("main".into()),
            },
            command: "cargo check".into(),
            last_activity: "checking workspace".into(),
            notification: None,
        },
        SessionSummary {
            id: SessionId::new("claude-ui"),
            title: "UI polish pass".into(),
            agent: AgentKind::Claude,
            status: SessionStatus::Waiting,
            workspace: WorkspaceRef {
                cwd: PathBuf::from("C:/Users/bunny/Downloads/lazyterm"),
                git_branch: Some("ui-spike".into()),
            },
            command: "claude".into(),
            last_activity: "reading terminal renderer".into(),
            notification: Some("branch has changes".into()),
        },
    ]
}

impl Render for LazytermApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .bg(rgb(APP_BG))
            .child(self.render_sidebar())
            .child(self.render_terminal_panel())
    }
}
