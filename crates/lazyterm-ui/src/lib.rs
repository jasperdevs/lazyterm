use gpui::{
    div, img, prelude::*, px, rgb, App, ClipboardItem, Context, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, ParentElement, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement, Styled, Window,
};
use lazyterm_core::{AgentKind, SessionId, SessionStatus, SessionSummary, WorkspaceRef};
use lazyterm_pty::{terminal_size_to_pty_size, PtyHandle, PtySession, ShellCommand};
use lazyterm_terminal::TerminalSize;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

const BG: u32 = 0x080808;
const SIDEBAR: u32 = 0x171717;
const SURFACE: u32 = 0x111111;
const ROW_ACTIVE: u32 = 0x202020;
const BORDER: u32 = 0x262626;
const BORDER_ACTIVE: u32 = 0x6a6a6a;
const TEXT: u32 = 0xf2f2f2;
const TEXT_SOFT: u32 = 0xc9c9c9;
const TEXT_MUTED: u32 = 0x858585;
const TEXT_DIM: u32 = 0x5f5f5f;

const TITLEBAR_HEIGHT: f32 = 32.0;
const WORKSPACE_BAR_HEIGHT: f32 = 38.0;
const SIDEBAR_WIDTH: f32 = 288.0;
const SIDEBAR_COMPACT_WIDTH: f32 = 76.0;
const SETTINGS_PANEL_WIDTH: f32 = 320.0;
const TERMINAL_X_PADDING: f32 = 20.0;
const TERMINAL_Y_PADDING: f32 = 16.0;
const TERMINAL_CHAR_WIDTH: f32 = 8.0;
const TERMINAL_LINE_HEIGHT: f32 = 18.0;

pub struct LazytermApp {
    focus_handle: FocusHandle,
    cwd: PathBuf,
    branch: Option<String>,
    sessions: Vec<TerminalSession>,
    active_session: usize,
    poller_started: bool,
    settings_open: bool,
    ui_settings: UiSettings,
}

struct UiSettings {
    compact_tabs: bool,
    show_session_meta: bool,
    tile_sessions: bool,
    terminal_font_size: f32,
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
            settings_open: false,
            ui_settings: UiSettings {
                compact_tabs: false,
                show_session_meta: true,
                tile_sessions: false,
                terminal_font_size: 12.0,
            },
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
        let size = terminal_size_for_viewport(
            viewport,
            self.ui_settings.compact_tabs,
            self.ui_settings.terminal_font_size,
        );
        for session in &mut self.sessions {
            session.resize(size);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let handled = self.handle_app_key(event, cx) || self.write_key_to_active_pty(event);
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn handle_app_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let primary = modifiers.platform || (modifiers.control && modifiers.shift);

        if self.settings_open && key == "escape" {
            self.settings_open = false;
            return true;
        }

        if primary {
            match key {
                "t" => {
                    self.create_terminal();
                    return true;
                }
                "w" => {
                    self.close_active_terminal();
                    return true;
                }
                "r" => {
                    self.restart_active_terminal();
                    return true;
                }
                "v" => {
                    self.paste_clipboard(cx);
                    return true;
                }
                "c" => {
                    self.copy_active_transcript(cx);
                    return true;
                }
                "," => {
                    self.toggle_settings();
                    return true;
                }
                "b" => {
                    self.toggle_tile_sessions();
                    return true;
                }
                "+" | "=" => {
                    self.adjust_font_size(1.0);
                    return true;
                }
                "-" => {
                    self.adjust_font_size(-1.0);
                    return true;
                }
                _ => {}
            }

            if let Some(index) = tab_index_for_key(key) {
                self.activate_session(index);
                return true;
            }
        }

        if modifiers.control && key == "tab" {
            if modifiers.shift {
                self.activate_previous_session();
            } else {
                self.activate_next_session();
            }
            return true;
        }

        false
    }

    fn write_key_to_active_pty(&mut self, event: &KeyDownEvent) -> bool {
        let bytes = match event.keystroke.key.as_str() {
            "enter" => Some(b"\r".as_slice()),
            "backspace" => Some(b"\x7f".as_slice()),
            "escape" => Some(b"\x1b".as_slice()),
            "tab" if event.keystroke.modifiers.shift => Some(b"\x1b[Z".as_slice()),
            "tab" => Some(b"\t".as_slice()),
            "delete" => Some(b"\x1b[3~".as_slice()),
            "left" => Some(b"\x1b[D".as_slice()),
            "right" => Some(b"\x1b[C".as_slice()),
            "up" => Some(b"\x1b[A".as_slice()),
            "down" => Some(b"\x1b[B".as_slice()),
            "home" => Some(b"\x1b[H".as_slice()),
            "end" => Some(b"\x1b[F".as_slice()),
            "pageup" => Some(b"\x1b[5~".as_slice()),
            "pagedown" => Some(b"\x1b[6~".as_slice()),
            "insert" => Some(b"\x1b[2~".as_slice()),
            "f1" => Some(b"\x1bOP".as_slice()),
            "f2" => Some(b"\x1bOQ".as_slice()),
            "f3" => Some(b"\x1bOR".as_slice()),
            "f4" => Some(b"\x1bOS".as_slice()),
            "f5" => Some(b"\x1b[15~".as_slice()),
            "f6" => Some(b"\x1b[17~".as_slice()),
            "f7" => Some(b"\x1b[18~".as_slice()),
            "f8" => Some(b"\x1b[19~".as_slice()),
            "f9" => Some(b"\x1b[20~".as_slice()),
            "f10" => Some(b"\x1b[21~".as_slice()),
            "f11" => Some(b"\x1b[23~".as_slice()),
            "f12" => Some(b"\x1b[24~".as_slice()),
            _ => None,
        };

        if let Some(bytes) = bytes {
            self.write_bytes_to_active_pty(bytes);
            return true;
        }

        let modifiers = event.keystroke.modifiers;
        if modifiers.platform || modifiers.function {
            return false;
        }

        if modifiers.control {
            if let Some(byte) = control_byte_for_key(event.keystroke.key.as_str()) {
                self.write_bytes_to_active_pty(&[byte]);
                return true;
            }
            return false;
        }

        let Some(input) = event.keystroke.key_char.as_ref() else {
            return false;
        };

        if modifiers.alt {
            self.write_bytes_to_active_pty(b"\x1b");
        }
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

    fn close_active_terminal(&mut self) {
        if self.sessions.len() == 1 {
            self.restart_active_terminal();
            return;
        }

        self.sessions.remove(self.active_session);
        if self.active_session >= self.sessions.len() {
            self.active_session = self.sessions.len() - 1;
        }
    }

    fn restart_active_terminal(&mut self) {
        let index = self.active_session + 1;
        let title = self.sessions[self.active_session].summary.title.clone();
        self.sessions[self.active_session] =
            TerminalSession::spawn(index, self.cwd.clone(), self.branch.clone(), title);
    }

    fn activate_session(&mut self, index: usize) {
        if index < self.sessions.len() {
            self.active_session = index;
        }
    }

    fn activate_next_session(&mut self) {
        if !self.sessions.is_empty() {
            self.active_session = (self.active_session + 1) % self.sessions.len();
        }
    }

    fn activate_previous_session(&mut self) {
        if self.sessions.is_empty() {
            return;
        }

        self.active_session = if self.active_session == 0 {
            self.sessions.len() - 1
        } else {
            self.active_session - 1
        };
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };

        self.write_bytes_to_active_pty(text.as_bytes());
    }

    fn copy_active_transcript(&self, cx: &mut Context<Self>) {
        let session = self.active_session();
        let mut transcript = String::new();
        for line in &session.lines {
            transcript.push_str(&line.text);
            transcript.push('\n');
        }
        if !session.pending_line.is_empty() {
            transcript.push_str(&session.pending_line);
        }

        if !transcript.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(transcript));
        }
    }

    fn adjust_font_size(&mut self, delta: f32) {
        self.ui_settings.terminal_font_size =
            (self.ui_settings.terminal_font_size + delta).clamp(10.0, 16.0);
    }

    fn toggle_tile_sessions(&mut self) {
        self.ui_settings.tile_sessions = !self.ui_settings.tile_sessions;
    }

    fn focus_terminal(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    fn active_session(&self) -> &TerminalSession {
        &self.sessions[self.active_session]
    }

    fn workspace_label(&self) -> String {
        self.cwd
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| self.cwd.display().to_string())
    }

    fn toggle_settings(&mut self) {
        self.settings_open = !self.settings_open;
    }

    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(TITLEBAR_HEIGHT))
            .px_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SIDEBAR))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .font_family("JetBrains Mono")
                    .child(
                        div()
                            .size(px(20.0))
                            .rounded_lg()
                            .overflow_hidden()
                            .child(img("logoblackbackground.png").size_full()),
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
                    .gap_1()
                    .child(self.render_titlebar_button(
                        "set",
                        "titlebar-settings",
                        cx,
                        |this, _| {
                            this.toggle_settings();
                        },
                    ))
                    .child(
                        self.render_titlebar_button("x", "window-close", cx, |_, window| {
                            window.remove_window();
                        }),
                    ),
            )
    }

    fn render_titlebar_button(
        &self,
        label: &'static str,
        id: &'static str,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window) + 'static,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(28.0))
            .h(px(24.0))
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .text_color(rgb(TEXT_MUTED))
            .font_family("JetBrains Mono")
            .text_size(px(12.0))
            .child(label)
            .id(id)
            .on_click(cx.listener(move |this, _, window, cx| {
                action(this, window);
                this.focus_terminal(window, cx);
                cx.notify();
            }))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = div().flex().flex_col().gap_2();
        for (index, session) in self.sessions.iter().enumerate() {
            tabs = tabs.child(self.render_session_tab(session, index, cx));
        }
        let width = if self.ui_settings.compact_tabs {
            SIDEBAR_COMPACT_WIDTH
        } else {
            SIDEBAR_WIDTH
        };

        div()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(width))
            .h_full()
            .border_r_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SIDEBAR))
            .when(!self.ui_settings.compact_tabs, |this| {
                this.child(self.render_sidebar_header())
            })
            .when(self.ui_settings.compact_tabs, |this| {
                this.items_center().px_2().py_2().child(
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
                        .text_size(px(11.0))
                        .child("new")
                        .id("new-terminal")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.create_terminal();
                            this.focus_terminal(window, cx);
                            cx.notify();
                        })),
                )
            })
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .px_2()
                    .pb_2()
                    .id("session-rail")
                    .overflow_y_scroll()
                    .child(tabs.w_full()),
            )
            .when(!self.ui_settings.compact_tabs, |this| {
                this.child(self.render_session_actions(cx))
            })
    }

    fn render_sidebar_header(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_start()
            .h(px(54.0))
            .px_3()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .text_color(rgb(TEXT))
                            .font_family("JetBrains Mono")
                            .text_size(px(13.0))
                            .child(SharedString::from(self.workspace_label())),
                    )
                    .child(
                        div()
                            .text_color(rgb(TEXT_DIM))
                            .font_family("JetBrains Mono")
                            .text_size(px(11.0))
                            .child(SharedString::from(format!(
                                "{} shells",
                                self.sessions.len()
                            ))),
                    ),
            )
    }

    fn render_session_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .pb_2()
            .child(
                self.render_sidebar_action("new", "sidebar-action-new", cx, |this| {
                    this.create_terminal();
                }),
            )
            .child(
                self.render_sidebar_action("restart", "sidebar-action-restart", cx, |this| {
                    this.restart_active_terminal();
                }),
            )
            .child(
                self.render_sidebar_action("close", "sidebar-action-close", cx, |this| {
                    this.close_active_terminal();
                }),
            )
            .child(
                self.render_sidebar_action("tile", "sidebar-action-tile", cx, |this| {
                    this.toggle_tile_sessions();
                }),
            )
    }

    fn render_sidebar_action(
        &self,
        label: &'static str,
        id: &'static str,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self) + 'static,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(28.0))
            .px_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .text_color(rgb(TEXT_MUTED))
            .font_family("JetBrains Mono")
            .text_size(px(11.0))
            .child(label)
            .id(id)
            .on_click(cx.listener(move |this, _, window, cx| {
                action(this);
                this.focus_terminal(window, cx);
                cx.notify();
            }))
    }

    fn render_session_tab(
        &self,
        session: &TerminalSession,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = index == self.active_session;
        let status_color = match session.summary.status {
            SessionStatus::Failed => BORDER_ACTIVE,
            SessionStatus::Done => TEXT_DIM,
            _ => TEXT_SOFT,
        };
        let status_label = session.summary.status.label();

        if self.ui_settings.compact_tabs {
            return div()
                .flex()
                .items_center()
                .justify_center()
                .relative()
                .w_full()
                .h(px(50.0))
                .rounded_lg()
                .border_1()
                .border_color(rgb(if active { TEXT_SOFT } else { BORDER }))
                .bg(rgb(if active { ROW_ACTIVE } else { SIDEBAR }))
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
                }));
        }

        div()
            .flex()
            .items_center()
            .gap_3()
            .w_full()
            .min_h(px(68.0))
            .rounded_lg()
            .border_1()
            .border_color(rgb(if active { BORDER_ACTIVE } else { SIDEBAR }))
            .bg(rgb(if active { ROW_ACTIVE } else { SIDEBAR }))
            .font_family("JetBrains Mono")
            .px_3()
            .py_2()
            .child(
                div()
                    .w(px(3.0))
                    .h(px(44.0))
                    .rounded_full()
                    .bg(rgb(if active { TEXT_SOFT } else { status_color })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .text_color(rgb(if active { TEXT } else { TEXT_SOFT }))
                            .text_size(px(if active { 13.0 } else { 12.0 }))
                            .child(SharedString::from(session.summary.title.clone())),
                    )
                    .when(self.ui_settings.show_session_meta, |this| {
                        this.child(
                            div()
                                .text_color(rgb(if active { TEXT_MUTED } else { TEXT_DIM }))
                                .text_size(px(11.0))
                                .child(SharedString::from(status_label.to_string())),
                        )
                    }),
            )
            .id(format!("session-tab-{index}"))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.active_session = index;
                this.focus_terminal(window, cx);
                cx.notify();
            }))
    }

    fn render_terminal_workspace(&self, focused: bool, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .h_full()
            .when(self.ui_settings.tile_sessions, |this| {
                this.child(self.render_tiled_terminals(cx))
            })
            .when(!self.ui_settings.tile_sessions, |this| {
                this.child(self.render_terminal(self.active_session, focused, cx))
            })
    }

    fn render_tiled_terminals(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = if self.sessions.len() <= 1 { 1 } else { 2 };
        let mut tiles = div()
            .grid()
            .grid_cols(columns)
            .gap_2()
            .p_2()
            .flex_1()
            .h_full()
            .bg(rgb(BG));

        for index in 0..self.sessions.len() {
            tiles = tiles.child(self.render_terminal_tile(index, cx));
        }

        tiles
    }

    fn render_terminal_tile(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .min_h(px(180.0))
            .border_1()
            .border_color(rgb(if index == self.active_session {
                BORDER_ACTIVE
            } else {
                BORDER
            }))
            .rounded_lg()
            .overflow_hidden()
            .child(self.render_terminal(index, index == self.active_session, cx))
            .id(format!("terminal-tile-{index}"))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.active_session = index;
                this.focus_terminal(window, cx);
                cx.notify();
            }))
    }

    fn render_terminal(
        &self,
        session_index: usize,
        focused: bool,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let session = &self.sessions[session_index];
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
            .child(self.render_workspace_bar(session_index))
            .child(
                div()
                    .flex_1()
                    .px(px(TERMINAL_X_PADDING))
                    .py(px(TERMINAL_Y_PADDING))
                    .font_family("JetBrains Mono")
                    .text_size(px(self.ui_settings.terminal_font_size))
                    .line_height(px(terminal_line_height(
                        self.ui_settings.terminal_font_size,
                    )))
                    .id("terminal-transcript")
                    .overflow_y_scroll()
                    .child(transcript),
            )
            .child(self.render_statusline(focused))
    }

    fn render_workspace_bar(&self, session_index: usize) -> impl IntoElement {
        let session = &self.sessions[session_index];
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(WORKSPACE_BAR_HEIGHT))
            .px_3()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .font_family("JetBrains Mono")
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .size(px(18.0))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(BORDER_ACTIVE))
                            .bg(rgb(BG)),
                    )
                    .child(
                        div()
                            .text_color(rgb(TEXT_SOFT))
                            .text_size(px(12.0))
                            .child(SharedString::from(session_context_label(session))),
                    ),
            )
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
            .text_size(px(self.ui_settings.terminal_font_size))
            .line_height(px(terminal_line_height(
                self.ui_settings.terminal_font_size,
            )))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(color))
                    .child(SharedString::from(line.text.clone())),
            )
    }

    fn render_statusline(&self, focused: bool) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(24.0))
            .border_t_1()
            .border_color(rgb(if focused { BORDER_ACTIVE } else { BORDER }))
            .bg(rgb(SURFACE))
            .px_3()
            .font_family("JetBrains Mono")
            .text_size(px(11.0))
            .id("terminal-statusline")
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(rgb(if focused { TEXT } else { TEXT_DIM }))
                    .child(SharedString::from(
                        self.active_session().summary.command.clone(),
                    ))
                    .child(div().size(px(3.0)).rounded_full().bg(rgb(TEXT_DIM)))
                    .child(SharedString::from(
                        self.active_session().summary.status.label(),
                    )),
            )
    }

    fn render_settings_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .w(px(SETTINGS_PANEL_WIDTH))
            .h_full()
            .border_l_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SIDEBAR))
            .font_family("JetBrains Mono")
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(WORKSPACE_BAR_HEIGHT))
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .text_color(rgb(TEXT))
                            .text_size(px(12.0))
                            .child("Settings"),
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
                            .bg(rgb(BG))
                            .text_color(rgb(TEXT_MUTED))
                            .text_size(px(12.0))
                            .child("x")
                            .id("settings-close")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.settings_open = false;
                                this.focus_terminal(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .child(self.render_toggle_row(
                        "Compact tabs",
                        self.ui_settings.compact_tabs,
                        "settings-compact-tabs",
                        cx,
                        |this| {
                            this.ui_settings.compact_tabs = !this.ui_settings.compact_tabs;
                        },
                    ))
                    .child(self.render_toggle_row(
                        "Session metadata",
                        self.ui_settings.show_session_meta,
                        "settings-session-meta",
                        cx,
                        |this| {
                            this.ui_settings.show_session_meta =
                                !this.ui_settings.show_session_meta;
                        },
                    ))
                    .child(self.render_toggle_row(
                        "Tile sessions",
                        self.ui_settings.tile_sessions,
                        "settings-tile-sessions",
                        cx,
                        |this| {
                            this.ui_settings.tile_sessions = !this.ui_settings.tile_sessions;
                        },
                    ))
                    .child(self.render_font_size_row(cx)),
            )
    }

    fn render_toggle_row(
        &self,
        label: &'static str,
        active: bool,
        id: &'static str,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self) + 'static,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(40.0))
            .px_3()
            .rounded_lg()
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .text_color(rgb(TEXT_SOFT))
                    .text_size(px(12.0))
                    .child(label),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(42.0))
                    .h(px(24.0))
                    .rounded_lg()
                    .bg(rgb(if active { ROW_ACTIVE } else { BG }))
                    .border_1()
                    .border_color(rgb(if active { BORDER_ACTIVE } else { BORDER }))
                    .text_color(rgb(if active { TEXT } else { TEXT_DIM }))
                    .text_size(px(11.0))
                    .child(if active { "on" } else { "off" }),
            )
            .id(id)
            .on_click(cx.listener(move |this, _, window, cx| {
                action(this);
                this.focus_terminal(window, cx);
                cx.notify();
            }))
    }

    fn render_font_size_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(40.0))
            .px_3()
            .rounded_lg()
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .text_color(rgb(TEXT_SOFT))
                    .text_size(px(12.0))
                    .child("Font size"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(self.render_font_button("-", "font-size-down", cx, -1.0))
                    .child(
                        div()
                            .w(px(32.0))
                            .text_color(rgb(TEXT_MUTED))
                            .text_size(px(11.0))
                            .child(format!("{:.0}", self.ui_settings.terminal_font_size)),
                    )
                    .child(self.render_font_button("+", "font-size-up", cx, 1.0)),
            )
    }

    fn render_font_button(
        &self,
        label: &'static str,
        id: &'static str,
        cx: &mut Context<Self>,
        delta: f32,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(24.0))
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
            .text_color(rgb(TEXT_SOFT))
            .text_size(px(12.0))
            .child(label)
            .id(id)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.adjust_font_size(delta);
                this.focus_terminal(window, cx);
                cx.notify();
            }))
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
                    .relative()
                    .overflow_hidden()
                    .child(self.render_sidebar(cx))
                    .child(self.render_terminal_workspace(self.focus_handle.is_focused(window), cx))
                    .when(self.settings_open, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(0.0))
                                .right(px(0.0))
                                .bottom(px(0.0))
                                .child(self.render_settings_panel(cx)),
                        )
                    }),
            )
            .when(!self.focus_handle.is_focused(window), |this| {
                this.border_1().border_color(rgb(BORDER))
            })
    }
}

fn terminal_size_for_viewport(
    viewport: Size<Pixels>,
    compact_tabs: bool,
    terminal_font_size: f32,
) -> TerminalSize {
    let width = viewport.width.as_f32();
    let height = viewport.height.as_f32();
    let sidebar_width = if compact_tabs {
        SIDEBAR_COMPACT_WIDTH
    } else {
        SIDEBAR_WIDTH
    };
    let terminal_width = (width - sidebar_width - (TERMINAL_X_PADDING * 2.0)).max(160.0);
    let terminal_height =
        (height - TITLEBAR_HEIGHT - WORKSPACE_BAR_HEIGHT - 24.0 - (TERMINAL_Y_PADDING * 2.0))
            .max(96.0);
    let columns = (terminal_width / terminal_char_width(terminal_font_size))
        .floor()
        .max(20.0) as u16;
    let rows = (terminal_height / terminal_line_height(terminal_font_size))
        .floor()
        .max(5.0) as u16;

    TerminalSize::new(columns, rows)
}

fn terminal_char_width(font_size: f32) -> f32 {
    TERMINAL_CHAR_WIDTH * (font_size / 12.0)
}

fn terminal_line_height(font_size: f32) -> f32 {
    TERMINAL_LINE_HEIGHT * (font_size / 12.0)
}

fn control_byte_for_key(key: &str) -> Option<u8> {
    let byte = key.as_bytes().first().copied()?.to_ascii_lowercase();
    match byte {
        b'a'..=b'z' => Some(byte - b'a' + 1),
        b'[' => Some(0x1b),
        b'\\' => Some(0x1c),
        b']' => Some(0x1d),
        b'^' => Some(0x1e),
        b'_' => Some(0x1f),
        _ => None,
    }
}

fn tab_index_for_key(key: &str) -> Option<usize> {
    let value = key.parse::<usize>().ok()?;
    if (1..=9).contains(&value) {
        Some(value - 1)
    } else {
        None
    }
}

fn session_context_label(session: &TerminalSession) -> String {
    match session.summary.workspace.git_branch.as_deref() {
        Some(branch) => format!("{}  /  {branch}", session.summary.title),
        None => session.summary.title.clone(),
    }
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
    fn maps_common_control_keys_to_terminal_bytes() {
        assert_eq!(control_byte_for_key("a"), Some(0x01));
        assert_eq!(control_byte_for_key("c"), Some(0x03));
        assert_eq!(control_byte_for_key("l"), Some(0x0c));
        assert_eq!(control_byte_for_key("z"), Some(0x1a));
        assert_eq!(control_byte_for_key("1"), None);
    }

    #[test]
    fn maps_number_shortcuts_to_session_indexes() {
        assert_eq!(tab_index_for_key("1"), Some(0));
        assert_eq!(tab_index_for_key("9"), Some(8));
        assert_eq!(tab_index_for_key("0"), None);
        assert_eq!(tab_index_for_key("t"), None);
    }

    #[test]
    fn terminal_size_tracks_available_viewport() {
        let size = terminal_size_for_viewport(
            Size {
                width: px(1180.0),
                height: px(760.0),
            },
            false,
            12.0,
        );

        assert!(size.columns >= 90);
        assert!(size.rows >= 20);
    }

    #[test]
    fn terminal_size_keeps_small_windows_usable() {
        let size = terminal_size_for_viewport(
            Size {
                width: px(300.0),
                height: px(240.0),
            },
            false,
            12.0,
        );

        assert_eq!(size.columns, 20);
        assert_eq!(size.rows, 6);
    }

    #[test]
    fn terminal_size_tracks_font_size() {
        let default_font = terminal_size_for_viewport(
            Size {
                width: px(1180.0),
                height: px(760.0),
            },
            false,
            12.0,
        );
        let larger_font = terminal_size_for_viewport(
            Size {
                width: px(1180.0),
                height: px(760.0),
            },
            false,
            16.0,
        );

        assert!(larger_font.columns < default_font.columns);
        assert!(larger_font.rows < default_font.rows);
    }
}
