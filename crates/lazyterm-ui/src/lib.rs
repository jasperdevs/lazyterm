use gpui::{
    div, prelude::*, px, rgb, svg, App, Context, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    ParentElement, Pixels, Render, SharedString, Size, StatefulInteractiveElement, Styled, Window,
};
use lazyterm_core::{AgentKind, SessionId, SessionStatus, SessionSummary, WorkspaceRef};
use lazyterm_pty::{terminal_size_to_pty_size, PtyHandle, PtySession, ShellCommand};
use lazyterm_terminal::TerminalSize;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

const BG: u32 = 0x050505;
const PANEL: u32 = 0x0c0c0c;
const PANEL_ACTIVE: u32 = 0x181818;
const BORDER: u32 = 0x282828;
const BORDER_ACTIVE: u32 = 0x555555;
const TEXT: u32 = 0xf2f2f2;
const TEXT_SOFT: u32 = 0xc9c9c9;
const TEXT_MUTED: u32 = 0x858585;
const TEXT_DIM: u32 = 0x5f5f5f;

const TITLEBAR_HEIGHT: f32 = 38.0;
const RAIL_WIDTH: f32 = 76.0;
const TERMINAL_X_PADDING: f32 = 18.0;
const TERMINAL_Y_PADDING: f32 = 14.0;
const INPUT_HEIGHT: f32 = 42.0;
const TERMINAL_CHAR_WIDTH: f32 = 8.0;
const TERMINAL_LINE_HEIGHT: f32 = 18.0;

pub struct LazytermApp {
    focus_handle: FocusHandle,
    cwd: PathBuf,
    branch: Option<String>,
    sessions: Vec<TerminalSession>,
    active_session: usize,
    poller_started: bool,
}

struct TerminalSession {
    summary: SessionSummary,
    pty: Option<PtyHandle>,
    events: Receiver<PtyEvent>,
    lines: Vec<TerminalLine>,
    pending_line: String,
    terminal_size: TerminalSize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TerminalLine {
    kind: TerminalLineKind,
    text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalLineKind {
    Output,
    Error,
}

enum PtyEvent {
    Output(String),
    Error(String),
    Exited,
}

impl LazytermApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let branch = current_branch();
        let sessions = vec![TerminalSession::spawn(
            1,
            cwd.clone(),
            branch.clone(),
            "shell 1",
        )];

        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            cwd,
            branch,
            sessions,
            active_session: 0,
            poller_started: false,
        }
    }

    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn start_poller(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.poller_started {
            return;
        }

        self.poller_started = true;
        let app = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;

                if app
                    .update_in(cx, |app, _window, cx| {
                        if app.poll_pty_events() {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            })
            .detach();
    }

    fn poll_pty_events(&mut self) -> bool {
        let mut changed = false;
        for session in &mut self.sessions {
            while let Ok(event) = session.events.try_recv() {
                changed = true;
                match event {
                    PtyEvent::Output(output) => session.push_output(output),
                    PtyEvent::Error(error) => {
                        session.lines.push(TerminalLine::error(error));
                        session.summary.status = SessionStatus::Failed;
                    }
                    PtyEvent::Exited => {
                        session.flush_pending_line();
                        session.summary.status = SessionStatus::Done;
                    }
                }
            }
        }
        changed
    }

    fn resize_sessions(&mut self, viewport: Size<Pixels>) {
        let size = terminal_size_for_viewport(viewport);
        for session in &mut self.sessions {
            session.resize(size);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let handled = self.write_key_to_active_pty(event);
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn write_key_to_active_pty(&mut self, event: &KeyDownEvent) -> bool {
        let bytes = match event.keystroke.key.as_str() {
            "enter" => Some(b"\r".as_slice()),
            "backspace" => Some(b"\x7f".as_slice()),
            "delete" => Some(b"\x1b[3~".as_slice()),
            "left" => Some(b"\x1b[D".as_slice()),
            "right" => Some(b"\x1b[C".as_slice()),
            "up" => Some(b"\x1b[A".as_slice()),
            "down" => Some(b"\x1b[B".as_slice()),
            "home" => Some(b"\x1b[H".as_slice()),
            "end" => Some(b"\x1b[F".as_slice()),
            "c" if event.keystroke.modifiers.control => Some(b"\x03".as_slice()),
            "d" if event.keystroke.modifiers.control => Some(b"\x04".as_slice()),
            "l" if event.keystroke.modifiers.control => Some(b"\x0c".as_slice()),
            _ => None,
        };

        if let Some(bytes) = bytes {
            self.write_bytes_to_active_pty(bytes);
            return true;
        }

        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return false;
        }

        let Some(input) = event.keystroke.key_char.as_ref() else {
            return false;
        };

        self.write_bytes_to_active_pty(input.as_bytes());
        true
    }

    fn write_bytes_to_active_pty(&mut self, bytes: &[u8]) {
        let session = &mut self.sessions[self.active_session];
        let Some(pty) = &mut session.pty else {
            return;
        };

        if let Err(error) = pty.write_all(bytes) {
            session
                .lines
                .push(TerminalLine::error(format!("write failed: {error}")));
            session.summary.status = SessionStatus::Failed;
        } else {
            session.summary.status = SessionStatus::Running;
        }
    }

    fn create_terminal(&mut self) {
        let index = self.sessions.len() + 1;
        self.sessions.push(TerminalSession::spawn(
            index,
            self.cwd.clone(),
            self.branch.clone(),
            format!("shell {index}"),
        ));
        self.active_session = self.sessions.len() - 1;
    }

    fn focus_terminal(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(TITLEBAR_HEIGHT))
            .px_3()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .font_family("JetBrains Mono")
                    .child(
                        div()
                            .size(px(24.0))
                            .rounded_lg()
                            .overflow_hidden()
                            .child(svg().path("logoblackbackground.svg").size_full()),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(TEXT))
                            .child("Lazyterm"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.0))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(PANEL))
                    .text_color(rgb(TEXT_MUTED))
                    .text_size(px(12.0))
                    .child("x")
                    .id("window-close")
                    .on_click(cx.listener(|_, _, window, _| window.remove_window())),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = div().flex().flex_col().gap_2();
        for (index, session) in self.sessions.iter().enumerate() {
            tabs = tabs.child(self.render_session_tab(session, index, cx));
        }

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .w(px(RAIL_WIDTH))
            .h_full()
            .px_2()
            .py_3()
            .border_r_1()
            .border_color(rgb(BORDER))
            .bg(rgb(PANEL))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(42.0))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BG))
                    .text_color(rgb(TEXT_SOFT))
                    .font_family("JetBrains Mono")
                    .text_size(px(16.0))
                    .child("+")
                    .id("new-terminal")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.create_terminal();
                        this.focus_terminal(window, cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .id("session-rail")
                    .overflow_y_scroll()
                    .child(tabs.w_full()),
            )
    }

    fn render_session_tab(
        &self,
        session: &TerminalSession,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = index == self.active_session;
        let status_color = match session.summary.status {
            SessionStatus::Failed => TEXT,
            SessionStatus::Done => TEXT_MUTED,
            _ => TEXT_SOFT,
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .relative()
            .w_full()
            .h(px(50.0))
            .rounded_lg()
            .border_1()
            .border_color(rgb(if active { BORDER_ACTIVE } else { BORDER }))
            .bg(rgb(if active { PANEL_ACTIVE } else { PANEL }))
            .font_family("JetBrains Mono")
            .child(
                div()
                    .absolute()
                    .left(px(6.0))
                    .top(px(6.0))
                    .size(px(5.0))
                    .rounded_full()
                    .bg(rgb(status_color)),
            )
            .child(
                div()
                    .text_color(rgb(if active { TEXT } else { TEXT_MUTED }))
                    .text_size(px(14.0))
                    .child((index + 1).to_string()),
            )
            .id(format!("session-tab-{index}"))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.active_session = index;
                this.focus_terminal(window, cx);
                cx.notify();
            }))
    }

    fn render_terminal(&self, focused: bool) -> impl IntoElement {
        let session = &self.sessions[self.active_session];
        let mut transcript = div().flex().flex_col().gap_1();

        for line in &session.lines {
            transcript = transcript.child(self.render_line(line));
        }

        if !session.pending_line.is_empty() {
            transcript = transcript
                .child(self.render_line(&TerminalLine::output(session.pending_line.clone())));
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .bg(rgb(BG))
            .child(
                div()
                    .flex_1()
                    .px(px(TERMINAL_X_PADDING))
                    .py(px(TERMINAL_Y_PADDING))
                    .font_family("JetBrains Mono")
                    .text_size(px(12.0))
                    .line_height(px(TERMINAL_LINE_HEIGHT))
                    .id("terminal-transcript")
                    .overflow_y_scroll()
                    .child(transcript),
            )
            .child(self.render_input(focused))
    }

    fn render_line(&self, line: &TerminalLine) -> impl IntoElement {
        let color = match line.kind {
            TerminalLineKind::Output => TEXT_SOFT,
            TerminalLineKind::Error => TEXT,
        };

        div()
            .flex()
            .items_start()
            .font_family("JetBrains Mono")
            .text_size(px(12.0))
            .line_height(px(TERMINAL_LINE_HEIGHT))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(color))
                    .child(SharedString::from(line.text.clone())),
            )
    }

    fn render_input(&self, focused: bool) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_3()
            .h(px(INPUT_HEIGHT))
            .border_t_1()
            .border_color(rgb(if focused { BORDER_ACTIVE } else { BORDER }))
            .bg(rgb(BG))
            .px(px(TERMINAL_X_PADDING))
            .font_family("JetBrains Mono")
            .text_size(px(13.0))
            .id("terminal-input")
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(14.0))
                    .text_color(rgb(if focused { TEXT } else { TEXT_DIM }))
                    .child(">"),
            )
            .child(div().flex_1().min_h(px(1.0)))
            .child(
                div()
                    .w(px(7.0))
                    .h(px(18.0))
                    .bg(rgb(if focused { TEXT } else { TEXT_DIM })),
            )
    }
}

impl TerminalSession {
    fn spawn(index: usize, cwd: PathBuf, branch: Option<String>, title: impl Into<String>) -> Self {
        let title = title.into();
        let (sender, events) = mpsc::channel();
        let mut command = ShellCommand::default_for_platform();
        command.cwd = Some(cwd.clone());
        let command_label = command.program.clone();
        let mut lines = Vec::new();

        let (pty, status) =
            match PtySession::spawn(&command, terminal_size_to_pty_size(TerminalSize::DEFAULT)) {
                Ok(session) => {
                    let (handle, mut reader) = session.split();
                    thread::Builder::new()
                        .name(format!("lazyterm-pty-reader-{index}"))
                        .spawn(move || {
                            let mut buffer = [0_u8; 4096];
                            loop {
                                match reader.read(&mut buffer) {
                                    Ok(0) => break,
                                    Ok(count) => {
                                        let output =
                                            String::from_utf8_lossy(&buffer[..count]).into_owned();
                                        if sender.send(PtyEvent::Output(output)).is_err() {
                                            return;
                                        }
                                    }
                                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                                    Err(error) => {
                                        let _ = sender.send(PtyEvent::Error(error.to_string()));
                                        break;
                                    }
                                }
                            }
                            let _ = sender.send(PtyEvent::Exited);
                        })
                        .expect("spawn PTY reader thread");
                    (Some(handle), SessionStatus::Running)
                }
                Err(error) => {
                    lines.push(TerminalLine::error(format!("pty spawn failed: {error}")));
                    (None, SessionStatus::Failed)
                }
            };

        Self {
            summary: SessionSummary {
                id: SessionId::new(format!("shell-{index}")),
                title,
                agent: AgentKind::Shell,
                status,
                workspace: WorkspaceRef {
                    cwd,
                    git_branch: branch,
                },
                command: command_label,
                last_activity: "attached".into(),
                notification: None,
            },
            pty,
            events,
            lines,
            pending_line: String::new(),
            terminal_size: TerminalSize::DEFAULT,
        }
    }

    fn push_output(&mut self, output: String) {
        let output = normalize_pty_output(&output);
        for segment in output.split_inclusive('\n') {
            if segment.ends_with('\n') {
                self.pending_line.push_str(segment.trim_end_matches('\n'));
                self.flush_pending_line();
            } else {
                self.pending_line.push_str(segment);
            }
        }
        self.summary.status = SessionStatus::Running;
    }

    fn flush_pending_line(&mut self) {
        if self.pending_line.is_empty() {
            return;
        }

        let text = std::mem::take(&mut self.pending_line);
        self.lines.push(TerminalLine::output(text));
        if self.lines.len() > 2_000 {
            let extra = self.lines.len() - 2_000;
            self.lines.drain(0..extra);
        }
    }

    fn resize(&mut self, size: TerminalSize) {
        if self.terminal_size == size {
            return;
        }

        self.terminal_size = size;
        let Some(pty) = &self.pty else {
            return;
        };

        if let Err(error) = pty.resize(terminal_size_to_pty_size(size)) {
            self.lines
                .push(TerminalLine::error(format!("resize failed: {error}")));
            self.summary.status = SessionStatus::Failed;
        }
    }
}

impl TerminalLine {
    fn output(text: impl Into<String>) -> Self {
        Self {
            kind: TerminalLineKind::Output,
            text: text.into(),
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            kind: TerminalLineKind::Error,
            text: text.into(),
        }
    }
}

impl Focusable for LazytermApp {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        LazytermApp::focus_handle(self, cx)
    }
}

impl Render for LazytermApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.start_poller(window, cx);
        self.poll_pty_events();
        self.resize_sessions(window.viewport_size());

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .id("lazyterm-root")
            .on_click(cx.listener(|this, _, window, cx| this.focus_terminal(window, cx)))
            .child(self.render_titlebar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_sidebar(cx))
                    .child(self.render_terminal(self.focus_handle.is_focused(window))),
            )
            .when(!self.focus_handle.is_focused(window), |this| {
                this.border_1().border_color(rgb(BORDER))
            })
    }
}

fn terminal_size_for_viewport(viewport: Size<Pixels>) -> TerminalSize {
    let width = viewport.width.as_f32();
    let height = viewport.height.as_f32();
    let terminal_width = (width - RAIL_WIDTH - (TERMINAL_X_PADDING * 2.0)).max(160.0);
    let terminal_height =
        (height - TITLEBAR_HEIGHT - INPUT_HEIGHT - (TERMINAL_Y_PADDING * 2.0)).max(96.0);
    let columns = (terminal_width / TERMINAL_CHAR_WIDTH).floor().max(20.0) as u16;
    let rows = (terminal_height / TERMINAL_LINE_HEIGHT).floor().max(5.0) as u16;

    TerminalSize::new(columns, rows)
}

fn normalize_pty_output(output: &str) -> String {
    let mut normalized = String::new();
    let mut chars = output.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    normalized.push('\n');
                }
            }
            '\x1b' => {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() || next == '~' {
                        break;
                    }
                }
            }
            '\u{8}' => {
                normalized.pop();
            }
            '\t' | '\n' => normalized.push(ch),
            ch if ch.is_control() => {}
            ch => normalized.push(ch),
        }
    }

    normalized
}

fn current_branch() -> Option<String> {
    std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .output()
        .ok()
        .and_then(|output| output.status.success().then_some(output.stdout))
        .and_then(|stdout| String::from_utf8(stdout).ok())
        .map(|branch| branch.trim().to_string())
        .filter(|branch| !branch.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_crlf_without_double_newlines() {
        assert_eq!(normalize_pty_output("one\r\ntwo\r\n"), "one\ntwo\n");
    }

    #[test]
    fn normalizes_carriage_returns_as_line_boundaries() {
        assert_eq!(normalize_pty_output("prompt\rnext"), "prompt\nnext");
    }

    #[test]
    fn strips_simple_ansi_control_sequences() {
        assert_eq!(normalize_pty_output("\x1b[31merror\x1b[0m"), "error");
    }

    #[test]
    fn applies_backspace_to_pending_text() {
        assert_eq!(normalize_pty_output("ab\u{8}c"), "ac");
    }

    #[test]
    fn terminal_size_tracks_available_viewport() {
        let size = terminal_size_for_viewport(Size {
            width: px(1180.0),
            height: px(760.0),
        });

        assert!(size.columns >= 100);
        assert!(size.rows >= 20);
    }

    #[test]
    fn terminal_size_keeps_small_windows_usable() {
        let size = terminal_size_for_viewport(Size {
            width: px(300.0),
            height: px(240.0),
        });

        assert_eq!(size.columns, 23);
        assert_eq!(size.rows, 7);
    }
}
