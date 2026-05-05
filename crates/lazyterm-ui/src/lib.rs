use alacritty_terminal::{
    event::{Event as AlacrittyEvent, EventListener},
    grid::Dimensions,
    term::{cell::Cell, cell::Flags, Config as AlacrittyConfig, Term as AlacrittyTerm},
    vte::ansi::{
        Color as AnsiColor, CursorShape, NamedColor as AnsiNamedColor, Processor as AnsiProcessor,
        Rgb as AnsiRgb,
    },
};
use gpui::{
    div, img, prelude::*, px, rgb, App, Bounds, ClipboardItem, Context, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, FontWeight,
    GlobalElementId, IntoElement, KeyDownEvent, Keystroke, LayoutId, ParentElement, Pixels, Point,
    Render, SharedString, Size, StatefulInteractiveElement, Style, Styled, Subscription,
    UTF16Selection, Window,
};
use lazyterm_core::{AgentKind, SessionId, SessionStatus, SessionSummary, WorkspaceRef};
use lazyterm_pty::{terminal_size_to_pty_size, PtyHandle, PtySession, ShellCommand};
use lazyterm_terminal::TerminalSize;
use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

const BG: u32 = 0x050505;
const SIDEBAR: u32 = 0x0d0d0d;
const SURFACE: u32 = 0x101010;
const ROW_ACTIVE: u32 = 0x242424;
const BORDER: u32 = 0x202020;
const BORDER_ACTIVE: u32 = 0x8f8f8f;
const TEXT: u32 = 0xf2f2f2;
const TEXT_SOFT: u32 = 0xc9c9c9;
const TEXT_MUTED: u32 = 0x858585;
const TEXT_DIM: u32 = 0x5f5f5f;
const TEXT_FAINT: u32 = 0x3f3f3f;

const TITLEBAR_HEIGHT: f32 = 32.0;
const WORKSPACE_BAR_HEIGHT: f32 = 38.0;
const SIDEBAR_WIDTH: f32 = 76.0;
const COMMAND_PALETTE_WIDTH: f32 = 320.0;
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
    initial_focus_done: bool,
    keystroke_observer: Option<Subscription>,
    command_palette_open: bool,
    command_palette_query: String,
    ui_settings: UiSettings,
}

struct UiSettings {
    tile_sessions: bool,
    terminal_font_size: f32,
}

struct TerminalSession {
    summary: SessionSummary,
    pty: Option<PtyHandle>,
    events: Receiver<PtyEvent>,
    terminal: TerminalGrid,
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
    Output(Vec<u8>),
    Error(String),
    Exited,
}

struct TerminalGrid {
    term: AlacrittyTerm<TerminalEventProxy>,
    parser: AnsiProcessor,
    pty_writes: Receiver<Vec<u8>>,
}

#[derive(Clone)]
struct TerminalEventProxy {
    pty_writes: Sender<Vec<u8>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TerminalGridRow {
    runs: Vec<TerminalCellRun>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TerminalCellStyle {
    foreground: u32,
    background: Option<u32>,
    bold: bool,
    dim: bool,
    underline: bool,
    cursor: CursorRender,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorRender {
    None,
    Block,
    Beam,
    Underline,
    Hollow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TerminalCellRun {
    text: String,
    style: TerminalCellStyle,
}

struct GridSize(TerminalSize);

struct TerminalInputElement {
    app: Entity<LazytermApp>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandKind {
    NewShell,
    NewCodex,
    NewClaude,
    NewOpenCode,
    SplitPane,
    ToggleLayout,
    RestartPane,
    ClosePane,
    CopyTranscript,
    Paste,
    FontDown,
    FontUp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommandItem {
    kind: CommandKind,
    label: &'static str,
    shortcut: &'static str,
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
            AgentKind::Shell,
        )];

        let mut app = Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            cwd,
            branch,
            sessions,
            active_session: 0,
            poller_started: false,
            initial_focus_done: false,
            keystroke_observer: None,
            command_palette_open: false,
            command_palette_query: String::new(),
            ui_settings: UiSettings {
                tile_sessions: false,
                terminal_font_size: 12.0,
            },
        };

        app.keystroke_observer = Some(cx.observe_keystrokes(|this, event, window, cx| {
            if this.focus_handle.is_focused(window) {
                return;
            }

            if this.handle_keystroke(&event.keystroke, cx)
                || this.write_keystroke_to_active_pty(&event.keystroke)
            {
                this.focus_terminal(window, cx);
                cx.notify();
            }
        }));

        app
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
                    PtyEvent::Output(output) => session.push_output(&output),
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
            self.ui_settings.terminal_font_size,
            self.sessions.len(),
            self.ui_settings.tile_sessions,
        );
        for session in &mut self.sessions {
            session.resize(size);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let handled = self.handle_keystroke(&event.keystroke, cx)
            || self.write_keystroke_to_active_pty(&event.keystroke);
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn handle_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let key = keystroke.key.as_str();
        let modifiers = keystroke.modifiers;
        let primary = modifiers.platform || (modifiers.control && modifiers.shift);

        if self.command_palette_open {
            match key {
                "escape" => {
                    self.command_palette_open = false;
                    self.command_palette_query.clear();
                    return true;
                }
                "backspace" => {
                    self.command_palette_query.pop();
                    return true;
                }
                "enter" => {
                    if let Some(command) = self.filtered_commands().first().copied() {
                        self.run_command(command.kind, cx);
                        self.command_palette_open = false;
                        self.command_palette_query.clear();
                    }
                    return true;
                }
                _ => {}
            }

            if !modifiers.control && !modifiers.alt && !modifiers.platform && !modifiers.function {
                if let Some(input) = keystroke.key_char.as_ref() {
                    self.command_palette_query.push_str(input);
                    return true;
                }
            }
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
                "p" | "k" | "," => {
                    self.toggle_command_palette();
                    return true;
                }
                "b" => {
                    self.split_workspace();
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

    fn write_keystroke_to_active_pty(&mut self, keystroke: &Keystroke) -> bool {
        let bytes = match keystroke.key.as_str() {
            "enter" => Some(b"\r".as_slice()),
            "backspace" => Some(b"\x7f".as_slice()),
            "escape" => Some(b"\x1b".as_slice()),
            "tab" if keystroke.modifiers.shift => Some(b"\x1b[Z".as_slice()),
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

        let modifiers = keystroke.modifiers;
        if modifiers.platform || modifiers.function {
            return false;
        }

        if modifiers.control {
            if let Some(byte) = control_byte_for_key(keystroke.key.as_str()) {
                self.write_bytes_to_active_pty(&[byte]);
                return true;
            }
            return false;
        }

        let Some(input) = keystroke.key_char.as_ref() else {
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
            AgentKind::Shell,
        ));
        self.active_session = self.sessions.len() - 1;
    }

    fn create_agent_terminal(&mut self, agent: AgentKind) {
        let index = self.sessions.len() + 1;
        let title = format!("{} {index}", agent.label().to_ascii_lowercase());
        self.sessions.push(TerminalSession::spawn(
            index,
            self.cwd.clone(),
            self.branch.clone(),
            title,
            agent,
        ));
        self.active_session = self.sessions.len() - 1;
    }

    fn close_active_terminal(&mut self) {
        if self.sessions.len() == 1 {
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
        let agent = self.sessions[self.active_session].summary.agent;
        self.sessions[self.active_session] =
            TerminalSession::spawn(index, self.cwd.clone(), self.branch.clone(), title, agent);
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

    fn split_workspace(&mut self) {
        if self.sessions.len() == 1 {
            self.create_terminal();
        }
        self.ui_settings.tile_sessions = true;
    }

    fn run_command(&mut self, command: CommandKind, cx: &mut Context<Self>) {
        match command {
            CommandKind::NewShell => self.create_terminal(),
            CommandKind::NewCodex => self.create_agent_terminal(AgentKind::Codex),
            CommandKind::NewClaude => self.create_agent_terminal(AgentKind::Claude),
            CommandKind::NewOpenCode => self.create_agent_terminal(AgentKind::OpenCode),
            CommandKind::SplitPane => self.split_workspace(),
            CommandKind::ToggleLayout => self.toggle_tile_sessions(),
            CommandKind::RestartPane => self.restart_active_terminal(),
            CommandKind::ClosePane => self.close_active_terminal(),
            CommandKind::CopyTranscript => self.copy_active_transcript(cx),
            CommandKind::Paste => self.paste_clipboard(cx),
            CommandKind::FontDown => self.adjust_font_size(-1.0),
            CommandKind::FontUp => self.adjust_font_size(1.0),
        }
    }

    fn commands(&self) -> Vec<CommandItem> {
        let layout_label = if self.ui_settings.tile_sessions {
            "single pane"
        } else {
            "tile panes"
        };

        vec![
            CommandItem {
                kind: CommandKind::NewShell,
                label: "new shell",
                shortcut: "ctrl+shift+t",
            },
            CommandItem {
                kind: CommandKind::NewCodex,
                label: "new codex",
                shortcut: "agent",
            },
            CommandItem {
                kind: CommandKind::NewClaude,
                label: "new claude",
                shortcut: "agent",
            },
            CommandItem {
                kind: CommandKind::NewOpenCode,
                label: "new opencode",
                shortcut: "agent",
            },
            CommandItem {
                kind: CommandKind::SplitPane,
                label: "split pane",
                shortcut: "ctrl+shift+b",
            },
            CommandItem {
                kind: CommandKind::ToggleLayout,
                label: layout_label,
                shortcut: "ctrl+shift+b",
            },
            CommandItem {
                kind: CommandKind::RestartPane,
                label: "restart pane",
                shortcut: "ctrl+shift+r",
            },
            CommandItem {
                kind: CommandKind::ClosePane,
                label: "close pane",
                shortcut: "ctrl+shift+w",
            },
            CommandItem {
                kind: CommandKind::CopyTranscript,
                label: "copy transcript",
                shortcut: "ctrl+shift+c",
            },
            CommandItem {
                kind: CommandKind::Paste,
                label: "paste",
                shortcut: "ctrl+shift+v",
            },
            CommandItem {
                kind: CommandKind::FontDown,
                label: "smaller font",
                shortcut: "ctrl+shift+-",
            },
            CommandItem {
                kind: CommandKind::FontUp,
                label: "larger font",
                shortcut: "ctrl+shift+=",
            },
        ]
    }

    fn filtered_commands(&self) -> Vec<CommandItem> {
        let query = self.command_palette_query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return self.commands();
        }

        self.commands()
            .into_iter()
            .filter(|command| command.label.contains(&query) || command.shortcut.contains(&query))
            .collect()
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

    fn toggle_command_palette(&mut self) {
        self.command_palette_open = !self.command_palette_open;
        if !self.command_palette_open {
            self.command_palette_query.clear();
        }
    }

    fn push_command_palette_text(&mut self, text: &str) {
        for character in text.chars() {
            match character {
                '\u{8}' | '\u{7f}' => {
                    self.command_palette_query.pop();
                }
                '\r' | '\n' => {}
                character if !character.is_control() => {
                    self.command_palette_query.push(character);
                }
                _ => {}
            }
        }
    }

    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(TITLEBAR_HEIGHT))
            .px_1()
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
                            .size(px(20.0))
                            .rounded_lg()
                            .overflow_hidden()
                            .child(img("logoblackbackground.png").size_full()),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(TEXT_SOFT))
                            .child(SharedString::from(self.workspace_label())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        self.render_titlebar_button("+", "titlebar-new", cx, |this, _| {
                            this.create_terminal();
                        }),
                    )
                    .child(self.render_titlebar_button(
                        if self.ui_settings.tile_sessions {
                            "1"
                        } else {
                            "2"
                        },
                        "titlebar-layout",
                        cx,
                        |this, _| {
                            if this.ui_settings.tile_sessions {
                                this.toggle_tile_sessions();
                            } else {
                                this.split_workspace();
                            }
                        },
                    ))
                    .child(
                        self.render_titlebar_button(":", "titlebar-settings", cx, |this, _| {
                            this.toggle_command_palette();
                        }),
                    )
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
            .w(px(28.0))
            .h(px(24.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
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

        div()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .border_r_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SIDEBAR))
            .items_center()
            .px_2()
            .py_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(42.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BG))
                    .text_color(rgb(TEXT_SOFT))
                    .font_family("JetBrains Mono")
                    .text_size(px(15.0))
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
                    .pb_2()
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
            SessionStatus::Failed => BORDER_ACTIVE,
            SessionStatus::Done => TEXT_DIM,
            _ => TEXT_SOFT,
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .relative()
            .w_full()
            .h(px(50.0))
            .rounded(px(8.0))
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
        let columns = tile_columns(self.sessions.len());
        let mut tiles = div()
            .grid()
            .grid_cols(columns as u16)
            .gap_0()
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
            .border_r_1()
            .border_b_1()
            .border_color(rgb(BORDER))
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
        let mut terminal_grid = div().flex().flex_col();
        for row in session.terminal.visible_rows() {
            terminal_grid = terminal_grid.child(self.render_grid_row(row));
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
                    .child(terminal_grid),
            )
            .child(self.render_statusline(session_index, focused))
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
            .bg(rgb(BG))
            .font_family("JetBrains Mono")
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.0))
                    .child(div().size(px(7.0)).rounded_full().bg(rgb(
                        if session_index == self.active_session {
                            TEXT_SOFT
                        } else {
                            TEXT_FAINT
                        },
                    )))
                    .child(
                        div()
                            .text_color(rgb(TEXT_MUTED))
                            .text_size(px(12.0))
                            .child(SharedString::from(session_context_label(session))),
                    ),
            )
    }

    fn render_grid_row(&self, row: TerminalGridRow) -> impl IntoElement {
        let mut row_element = div()
            .flex()
            .items_start()
            .font_family("JetBrains Mono")
            .text_size(px(self.ui_settings.terminal_font_size))
            .line_height(px(terminal_line_height(
                self.ui_settings.terminal_font_size,
            )))
            .text_color(rgb(TEXT_SOFT));

        for run in row.runs {
            row_element = row_element.child(self.render_cell_run(run));
        }

        row_element
    }

    fn render_cell_run(&self, run: TerminalCellRun) -> impl IntoElement {
        let mut run_element = div()
            .font_family("JetBrains Mono")
            .text_size(px(self.ui_settings.terminal_font_size))
            .line_height(px(terminal_line_height(
                self.ui_settings.terminal_font_size,
            )))
            .whitespace_nowrap()
            .text_color(rgb(run.style.foreground))
            .when_some(run.style.background, |this, background| {
                this.text_bg(rgb(background))
            })
            .when(run.style.bold, |this| this.font_weight(FontWeight::BOLD))
            .when(run.style.underline, |this| this.underline())
            .child(SharedString::from(run.text));

        run_element = match run.style.cursor {
            CursorRender::None => run_element,
            CursorRender::Block => run_element.text_bg(rgb(TEXT)).text_color(rgb(BG)),
            CursorRender::Beam => run_element.border_l_1().border_color(rgb(TEXT)),
            CursorRender::Underline => run_element.border_b_1().border_color(rgb(TEXT)),
            CursorRender::Hollow => run_element.border_1().border_color(rgb(TEXT_MUTED)),
        };

        run_element
    }

    fn render_statusline(&self, session_index: usize, focused: bool) -> impl IntoElement {
        let session = &self.sessions[session_index];
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
                    .child(SharedString::from(session.summary.command.clone()))
                    .child(div().size(px(3.0)).rounded_full().bg(rgb(TEXT_DIM)))
                    .child(SharedString::from(session.summary.status.label())),
            )
    }

    fn render_command_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut commands = div().flex().flex_col().gap_2();
        for command in self.filtered_commands() {
            commands = commands.child(self.render_command_action(command, cx));
        }
        if self.filtered_commands().is_empty() {
            commands = commands.child(
                div()
                    .min_h(px(36.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(7.0))
                    .bg(rgb(BG))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .text_color(rgb(TEXT_DIM))
                    .text_size(px(12.0))
                    .child("no command"),
            );
        }

        div()
            .flex()
            .flex_col()
            .w(px(COMMAND_PALETTE_WIDTH))
            .rounded(px(10.0))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .font_family("JetBrains Mono")
            .overflow_hidden()
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
                            .child("commands"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.0))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(BG))
                            .text_color(rgb(TEXT_MUTED))
                            .text_size(px(12.0))
                            .child("x")
                            .id("command-palette-close")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.command_palette_open = false;
                                this.focus_terminal(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .child(self.render_command_query())
                    .child(commands),
            )
    }

    fn render_command_action(
        &self,
        command: CommandItem,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .min_h(px(36.0))
            .px_3()
            .rounded(px(7.0))
            .bg(rgb(BG))
            .border_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .text_color(rgb(TEXT_SOFT))
                    .text_size(px(12.0))
                    .child(command.label),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(TEXT_DIM))
                    .text_size(px(11.0))
                    .child(command.shortcut),
            )
            .id(format!("command-{:?}", command.kind))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.run_command(command.kind, cx);
                this.command_palette_open = false;
                this.command_palette_query.clear();
                this.focus_terminal(window, cx);
                cx.notify();
            }))
    }

    fn render_command_query(&self) -> impl IntoElement {
        let text = if self.command_palette_query.is_empty() {
            "type command".into()
        } else {
            self.command_palette_query.clone()
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .min_h(px(36.0))
            .px_3()
            .rounded(px(7.0))
            .bg(rgb(BG))
            .border_1()
            .border_color(rgb(BORDER_ACTIVE))
            .child(
                div()
                    .text_color(rgb(if self.command_palette_query.is_empty() {
                        TEXT_DIM
                    } else {
                        TEXT
                    }))
                    .text_size(px(12.0))
                    .child(SharedString::from(text)),
            )
            .child(
                div()
                    .text_color(rgb(TEXT_DIM))
                    .text_size(px(11.0))
                    .child("enter"),
            )
    }
}

impl TerminalSession {
    fn spawn(
        index: usize,
        cwd: PathBuf,
        branch: Option<String>,
        title: impl Into<String>,
        agent: AgentKind,
    ) -> Self {
        let title = title.into();
        let (sender, events) = mpsc::channel();
        let mut command = command_for_agent(agent);
        command.cwd = Some(cwd.clone());
        let command_label = command_label(&command);
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
                                        let output = buffer[..count].to_vec();
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
                agent,
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
            terminal: TerminalGrid::new(TerminalSize::DEFAULT),
            lines,
            pending_line: String::new(),
            terminal_size: TerminalSize::DEFAULT,
        }
    }

    fn push_output(&mut self, output: &[u8]) {
        self.terminal.feed(output);
        self.flush_terminal_replies();
        let output = normalize_pty_output(&String::from_utf8_lossy(output));
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

    fn flush_terminal_replies(&mut self) {
        let replies = self.terminal.drain_pty_writes();
        let Some(pty) = &mut self.pty else {
            return;
        };

        for reply in replies {
            if let Err(error) = pty.write_all(&reply) {
                self.lines.push(TerminalLine::error(format!(
                    "terminal reply failed: {error}"
                )));
                self.summary.status = SessionStatus::Failed;
                break;
            }
        }
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
        self.terminal.resize(size);
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

impl TerminalGrid {
    fn new(size: TerminalSize) -> Self {
        let (sender, pty_writes) = mpsc::channel();
        Self {
            term: AlacrittyTerm::new(
                AlacrittyConfig::default(),
                &GridSize(size),
                TerminalEventProxy { pty_writes: sender },
            ),
            parser: AnsiProcessor::new(),
            pty_writes,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    fn drain_pty_writes(&mut self) -> Vec<Vec<u8>> {
        self.pty_writes.try_iter().collect()
    }

    fn resize(&mut self, size: TerminalSize) {
        self.term.resize(GridSize(size));
    }

    fn visible_rows(&self) -> Vec<TerminalGridRow> {
        let columns = self.term.grid().columns();
        let rows = self.term.grid().screen_lines();
        let content = self.term.renderable_content();
        let cursor = content.cursor;
        let mut visible = Vec::with_capacity(rows);
        let mut current = TerminalGridRow::new(columns);

        for indexed in content.display_iter {
            if indexed.point.column.0 == 0 && !current.runs.is_empty() {
                visible.push(current.trimmed());
                current = TerminalGridRow::new(columns);
            }

            let mut character = if indexed.cell.flags.intersects(
                Flags::HIDDEN | Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER,
            ) {
                ' '
            } else {
                indexed.cell.c
            };

            let cursor_render = cursor_render_at(cursor.shape, indexed.point == cursor.point);
            if cursor_render == CursorRender::Block
                && character == ' '
                && indexed.cell.flags.contains(Flags::HIDDEN)
            {
                character = ' ';
            }

            let style = terminal_cell_style(indexed.cell, cursor_render);
            current.push(character, style);
            if let Some(zerowidth) = indexed.cell.zerowidth() {
                for character in zerowidth {
                    current.push(*character, style);
                }
            }
        }

        visible.push(current.trimmed());
        while visible.len() < rows {
            visible.push(TerminalGridRow::default());
        }

        visible
    }
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: AlacrittyEvent) {
        if let AlacrittyEvent::PtyWrite(text) = event {
            let _ = self.pty_writes.send(text.into_bytes());
        }
    }
}

impl TerminalGridRow {
    fn new(columns: usize) -> Self {
        Self {
            runs: Vec::with_capacity(columns.min(120)),
        }
    }

    fn push(&mut self, character: char, style: TerminalCellStyle) {
        if let Some(run) = self.runs.last_mut() {
            if run.style == style {
                run.text.push(character);
                return;
            }
        }

        self.runs.push(TerminalCellRun {
            text: character.to_string(),
            style,
        });
    }

    fn trimmed(mut self) -> Self {
        while let Some(run) = self.runs.last_mut() {
            if run.style.background.is_some() || run.style.cursor != CursorRender::None {
                break;
            }

            let trimmed_len = run.text.trim_end_matches(' ').len();
            run.text.truncate(trimmed_len);
            if !run.text.is_empty() {
                break;
            }
            self.runs.pop();
        }

        self
    }

    #[cfg(test)]
    fn plain_text(&self) -> String {
        let mut text = String::new();
        for run in &self.runs {
            text.push_str(&run.text);
        }
        text
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        usize::from(self.0.rows)
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.0.rows)
    }

    fn columns(&self) -> usize {
        usize::from(self.0.columns)
    }
}

impl IntoElement for TerminalInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalInputElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = px(1.0).into();
        style.size.height = px(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.app.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.app.clone()),
            cx,
        );
    }
}

impl EntityInputHandler for LazytermApp {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        *adjusted_range = Some(0..0);
        Some(String::new())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.command_palette_open {
            self.push_command_palette_text(text);
        } else {
            self.write_bytes_to_active_pty(text.as_bytes());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.command_palette_open {
            self.push_command_palette_text(new_text);
        } else {
            self.write_bytes_to_active_pty(new_text.as_bytes());
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
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
        if !self.initial_focus_done {
            self.initial_focus_done = true;
            self.focus_terminal(window, cx);
        }
        self.poll_pty_events();
        self.resize_sessions(window.viewport_size());

        div()
            .flex()
            .flex_col()
            .size_full()
            .rounded(px(10.0))
            .overflow_hidden()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .id("lazyterm-root")
            .on_click(cx.listener(|this, _, window, cx| this.focus_terminal(window, cx)))
            .child(TerminalInputElement { app: cx.entity() })
            .child(self.render_titlebar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .relative()
                    .overflow_hidden()
                    .child(self.render_sidebar(cx))
                    .child(self.render_terminal_workspace(self.focus_handle.is_focused(window), cx))
                    .when(self.command_palette_open, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(44.0))
                                .right(px(12.0))
                                .child(self.render_command_palette(cx)),
                        )
                    }),
            )
            .when(self.focus_handle.is_focused(window), |this| {
                this.border_color(rgb(BORDER_ACTIVE))
            })
    }
}

fn terminal_size_for_viewport(
    viewport: Size<Pixels>,
    terminal_font_size: f32,
    session_count: usize,
    tiled: bool,
) -> TerminalSize {
    let width = viewport.width.as_f32();
    let height = viewport.height.as_f32();
    let panes = if tiled { session_count.max(1) } else { 1 };
    let tile_columns = tile_columns(panes) as f32;
    let tile_rows = ((panes as f32) / tile_columns).ceil().max(1.0);
    let terminal_width =
        ((width - SIDEBAR_WIDTH) / tile_columns - (TERMINAL_X_PADDING * 2.0)).max(160.0);
    let terminal_height = ((height - TITLEBAR_HEIGHT) / tile_rows
        - WORKSPACE_BAR_HEIGHT
        - 24.0
        - (TERMINAL_Y_PADDING * 2.0))
        .max(96.0);
    let columns = (terminal_width / terminal_char_width(terminal_font_size))
        .floor()
        .max(20.0) as u16;
    let rows = (terminal_height / terminal_line_height(terminal_font_size))
        .floor()
        .max(5.0) as u16;

    TerminalSize::new(columns, rows)
}

fn tile_columns(session_count: usize) -> usize {
    match session_count {
        0 | 1 => 1,
        2..=4 => 2,
        _ => 3,
    }
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
    let title = format!(
        "{}  /  {}",
        session.summary.title,
        session.summary.agent.label().to_ascii_lowercase()
    );

    match session.summary.workspace.git_branch.as_deref() {
        Some(branch) => format!("{title}  /  {branch}"),
        None => title,
    }
}

fn command_for_agent(agent: AgentKind) -> ShellCommand {
    match agent {
        AgentKind::Shell => ShellCommand::default_for_platform(),
        AgentKind::Codex => ShellCommand {
            program: "codex".into(),
            args: Vec::new(),
            cwd: None,
        },
        AgentKind::Claude => ShellCommand {
            program: "claude".into(),
            args: Vec::new(),
            cwd: None,
        },
        AgentKind::OpenCode => ShellCommand {
            program: "opencode".into(),
            args: Vec::new(),
            cwd: None,
        },
        AgentKind::Gemini => ShellCommand {
            program: "gemini".into(),
            args: Vec::new(),
            cwd: None,
        },
        AgentKind::Aider => ShellCommand {
            program: "aider".into(),
            args: Vec::new(),
            cwd: None,
        },
    }
}

fn command_label(command: &ShellCommand) -> String {
    if command.args.is_empty() {
        return command.program.clone();
    }

    format!("{} {}", command.program, command.args.join(" "))
}

fn terminal_cell_style(cell: &Cell, cursor: CursorRender) -> TerminalCellStyle {
    let mut foreground = terminal_color(cell.fg, false);
    let mut background = terminal_background(cell.bg);
    let bold = cell.flags.contains(Flags::BOLD);
    let dim = cell.flags.contains(Flags::DIM);

    if bold && matches!(cell.fg, AnsiColor::Named(AnsiNamedColor::Foreground)) {
        foreground = TEXT;
    }
    if dim {
        foreground = dim_color(foreground);
    }
    if cell.flags.contains(Flags::INVERSE) {
        let next_foreground = background.unwrap_or(BG);
        background = Some(foreground);
        foreground = next_foreground;
    }

    TerminalCellStyle {
        foreground,
        background,
        bold,
        dim,
        underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
        cursor,
    }
}

fn terminal_color(color: AnsiColor, background: bool) -> u32 {
    match color {
        AnsiColor::Named(named) => terminal_named_color(named, background),
        AnsiColor::Spec(rgb) => rgb_to_u32(rgb),
        AnsiColor::Indexed(index) => terminal_indexed_color(index),
    }
}

fn terminal_background(color: AnsiColor) -> Option<u32> {
    match color {
        AnsiColor::Named(AnsiNamedColor::Background) => None,
        AnsiColor::Named(named) => Some(terminal_named_color(named, true)),
        AnsiColor::Spec(rgb) => Some(rgb_to_u32(rgb)),
        AnsiColor::Indexed(index) => Some(terminal_indexed_color(index)),
    }
}

fn terminal_named_color(color: AnsiNamedColor, background: bool) -> u32 {
    match color {
        AnsiNamedColor::Foreground => TEXT_SOFT,
        AnsiNamedColor::Background => BG,
        AnsiNamedColor::BrightForeground => TEXT,
        AnsiNamedColor::DimForeground => TEXT_DIM,
        AnsiNamedColor::Cursor => TEXT,
        AnsiNamedColor::Black => 0x111111,
        AnsiNamedColor::Red => 0xd75f5f,
        AnsiNamedColor::Green => 0x87af87,
        AnsiNamedColor::Yellow => 0xd7af5f,
        AnsiNamedColor::Blue => 0x87afd7,
        AnsiNamedColor::Magenta => 0xaf87d7,
        AnsiNamedColor::Cyan => 0x87d7d7,
        AnsiNamedColor::White => 0xd0d0d0,
        AnsiNamedColor::BrightBlack => 0x6c6c6c,
        AnsiNamedColor::BrightRed => 0xff8787,
        AnsiNamedColor::BrightGreen => 0xafffaf,
        AnsiNamedColor::BrightYellow => 0xffff87,
        AnsiNamedColor::BrightBlue => 0xafd7ff,
        AnsiNamedColor::BrightMagenta => 0xd7afff,
        AnsiNamedColor::BrightCyan => 0xafffff,
        AnsiNamedColor::BrightWhite => 0xffffff,
        AnsiNamedColor::DimBlack => 0x080808,
        AnsiNamedColor::DimRed => 0x875f5f,
        AnsiNamedColor::DimGreen => 0x5f875f,
        AnsiNamedColor::DimYellow => 0x87875f,
        AnsiNamedColor::DimBlue => 0x5f5f87,
        AnsiNamedColor::DimMagenta => 0x875f87,
        AnsiNamedColor::DimCyan => 0x5f8787,
        AnsiNamedColor::DimWhite => 0x878787,
    }
    .max(if background { BG } else { 0 })
}

fn terminal_indexed_color(index: u8) -> u32 {
    const ANSI: [u32; 16] = [
        0x111111, 0xd75f5f, 0x87af87, 0xd7af5f, 0x87afd7, 0xaf87d7, 0x87d7d7, 0xd0d0d0, 0x6c6c6c,
        0xff8787, 0xafffaf, 0xffff87, 0xafd7ff, 0xd7afff, 0xafffff, 0xffffff,
    ];

    if let Some(color) = ANSI.get(usize::from(index)) {
        return *color;
    }

    if (16..=231).contains(&index) {
        let value = index - 16;
        let component = |step: u8| if step == 0 { 0 } else { 55 + (step * 40) };
        let red = component(value / 36);
        let green = component((value % 36) / 6);
        let blue = component(value % 6);
        return (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue);
    }

    let gray = 8 + ((index.saturating_sub(232)) * 10);
    (u32::from(gray) << 16) | (u32::from(gray) << 8) | u32::from(gray)
}

fn rgb_to_u32(color: AnsiRgb) -> u32 {
    (u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)
}

fn dim_color(color: u32) -> u32 {
    let red = ((color >> 16) & 0xff) / 2;
    let green = ((color >> 8) & 0xff) / 2;
    let blue = (color & 0xff) / 2;
    (red << 16) | (green << 8) | blue
}

fn cursor_render_at(shape: CursorShape, at_cursor: bool) -> CursorRender {
    if !at_cursor {
        return CursorRender::None;
    }

    match shape {
        CursorShape::Block => CursorRender::Block,
        CursorShape::Underline => CursorRender::Underline,
        CursorShape::Beam => CursorRender::Beam,
        CursorShape::HollowBlock => CursorRender::Hollow,
        CursorShape::Hidden => CursorRender::None,
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
    use alacritty_terminal::vte::ansi::{Color, Rgb};

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
    fn control_byte_for_key_accepts_uppercase_letters() {
        assert_eq!(control_byte_for_key("C"), Some(0x03));
    }

    #[test]
    fn agent_commands_map_to_cli_programs() {
        assert_eq!(command_for_agent(AgentKind::Codex).program, "codex");
        assert_eq!(command_for_agent(AgentKind::Claude).program, "claude");
        assert_eq!(command_for_agent(AgentKind::OpenCode).program, "opencode");
    }

    #[test]
    fn command_label_includes_arguments() {
        let command = ShellCommand {
            program: "pwsh.exe".into(),
            args: vec!["-NoLogo".into(), "-NoProfile".into()],
            cwd: None,
        };

        assert_eq!(command_label(&command), "pwsh.exe -NoLogo -NoProfile");
    }

    #[test]
    fn terminal_grid_preserves_truecolor_foreground() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));

        grid.feed(b"\x1b[38;2;10;20;30mX");

        let cell = grid
            .term
            .grid()
            .display_iter()
            .find(|indexed| indexed.cell.c == 'X')
            .expect("expected X cell");

        assert_eq!(
            cell.cell.fg,
            Color::Spec(Rgb {
                r: 10,
                g: 20,
                b: 30,
            })
        );
    }

    #[test]
    fn terminal_grid_preserves_bold_attribute() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));

        grid.feed(b"\x1b[1mX");

        let cell = grid
            .term
            .grid()
            .display_iter()
            .find(|indexed| indexed.cell.c == 'X')
            .expect("expected X cell");

        assert!(cell.cell.flags.contains(Flags::BOLD));
    }

    #[test]
    fn terminal_grid_preserves_underline_attribute() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));

        grid.feed(b"\x1b[4mX");

        let cell = grid
            .term
            .grid()
            .display_iter()
            .find(|indexed| indexed.cell.c == 'X')
            .expect("expected X cell");

        assert!(cell.cell.flags.contains(Flags::UNDERLINE));
    }

    #[test]
    fn maps_number_shortcuts_to_session_indexes() {
        assert_eq!(tab_index_for_key("1"), Some(0));
        assert_eq!(tab_index_for_key("9"), Some(8));
        assert_eq!(tab_index_for_key("0"), None);
        assert_eq!(tab_index_for_key("t"), None);
    }

    #[test]
    fn terminal_grid_tracks_cursor_movement_sequences() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));

        grid.feed(b"hello\x1b[1D!");
        let rows = grid.visible_rows();

        assert_eq!(rows[0].plain_text().trim_end(), "hell!");
        assert_eq!(
            rows[0].runs.last().map(|run| run.style.cursor),
            Some(CursorRender::Block)
        );
    }

    #[test]
    fn terminal_grid_honors_clear_screen_sequences() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));

        grid.feed(b"before\x1b[2J\x1b[Hafter");
        let rows = grid.visible_rows();

        assert_eq!(rows[0].plain_text().trim_end(), "after");
        assert!(rows.iter().skip(1).all(|row| row.runs.is_empty()));
    }

    #[test]
    fn terminal_grid_keeps_ansi_style_runs() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));

        grid.feed(b"\x1b[31mred\x1b[0m plain");
        let rows = grid.visible_rows();

        assert_eq!(rows[0].plain_text().trim_end(), "red plain");
        assert!(rows[0]
            .runs
            .iter()
            .any(|run| run.text == "red" && run.style.foreground == 0xd75f5f));
    }

    #[test]
    fn terminal_grid_keeps_inverse_and_underline_attributes() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));

        grid.feed(b"\x1b[7mrev\x1b[0m \x1b[4mul\x1b[0m");
        let rows = grid.visible_rows();

        assert_eq!(rows[0].plain_text().trim_end(), "rev ul");
        assert!(rows[0]
            .runs
            .iter()
            .any(|run| run.text == "rev" && run.style.background == Some(TEXT_SOFT)));
        assert!(rows[0]
            .runs
            .iter()
            .any(|run| run.text == "ul" && run.style.underline));
    }

    #[test]
    fn terminal_grid_replies_to_cursor_position_queries() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));

        grid.feed(b"\x1b[6n");
        let replies = grid.drain_pty_writes();

        assert_eq!(replies, vec![b"\x1b[1;1R".to_vec()]);
    }

    #[test]
    fn terminal_size_tracks_available_viewport() {
        let size = terminal_size_for_viewport(
            Size {
                width: px(1180.0),
                height: px(760.0),
            },
            12.0,
            1,
            false,
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
            12.0,
            1,
            false,
        );

        assert!(size.columns >= 20);
        assert_eq!(size.rows, 6);
    }

    #[test]
    fn terminal_size_tracks_font_size() {
        let default_font = terminal_size_for_viewport(
            Size {
                width: px(1180.0),
                height: px(760.0),
            },
            12.0,
            1,
            false,
        );
        let larger_font = terminal_size_for_viewport(
            Size {
                width: px(1180.0),
                height: px(760.0),
            },
            16.0,
            1,
            false,
        );

        assert!(larger_font.columns < default_font.columns);
        assert!(larger_font.rows < default_font.rows);
    }
}
