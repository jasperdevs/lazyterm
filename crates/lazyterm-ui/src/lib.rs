use alacritty_terminal::{
    event::{Event as AlacrittyEvent, EventListener},
    grid::{Dimensions, Scroll},
    term::{cell::Cell, cell::Flags, Config as AlacrittyConfig, Term as AlacrittyTerm, TermMode},
    vte::ansi::{
        Color as AnsiColor, CursorShape, NamedColor as AnsiNamedColor, Processor as AnsiProcessor,
        Rgb as AnsiRgb,
    },
};
use gpui::{
    div, img, prelude::*, px, relative, rgb, App, Bounds, ClipboardItem, Context, CursorStyle, Div,
    Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable,
    FontWeight, GlobalElementId, IntoElement, KeyDownEvent, Keystroke, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render,
    ScrollDelta, ScrollWheelEvent, SharedString, Size, StatefulInteractiveElement, Style, Styled,
    Subscription, UTF16Selection, Window, WindowControlArea,
};
use lazyterm_agents::{detect_status, AgentPreset, AGENT_PRESETS};
use lazyterm_api::{
    AgentHealthSummary, ApiRequest, ApiResponse, TerminalDensity as ApiTerminalDensity,
    TerminalRail as ApiTerminalRail, TileLayout as ApiTileLayout,
};
use lazyterm_core::{AgentKind, SessionId, SessionStatus, SessionSummary, WorkspaceRef};
use lazyterm_pty::{terminal_size_to_pty_size, PtyHandle, PtySession, ShellCommand};
use lazyterm_sessions::SessionStore;
use lazyterm_terminal::TerminalSize;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

const BG: u32 = 0x050505;
const SIDEBAR: u32 = 0x0d0d0d;
const SURFACE: u32 = 0x101010;
const SURFACE_ACTIVE: u32 = 0x181818;
const ROW_ACTIVE: u32 = 0x242424;
const BORDER: u32 = 0x202020;
const BORDER_ACTIVE: u32 = 0x8f8f8f;
const TEXT: u32 = 0xf2f2f2;
const TEXT_SOFT: u32 = 0xc9c9c9;
const TEXT_MUTED: u32 = 0x858585;
const TEXT_DIM: u32 = 0x5f5f5f;
const TEXT_FAINT: u32 = 0x3f3f3f;

const TITLEBAR_HEIGHT: f32 = 32.0;
const STATUSLINE_HEIGHT: f32 = 24.0;
const COMPACT_SIDEBAR_WIDTH: f32 = 76.0;
const DEFAULT_SIDEBAR_WIDTH: f32 = 212.0;
const WIDE_SIDEBAR_WIDTH: f32 = 268.0;
const COMMAND_PALETTE_WIDTH: f32 = 420.0;
const COMMAND_PALETTE_MAX_HEIGHT: f32 = 560.0;
const COMMAND_PALETTE_TOP: f32 = TITLEBAR_HEIGHT + 10.0;
const DEFAULT_TERMINAL_PADDING: f32 = 16.0;
const DEFAULT_SPLIT_RATIO: f32 = 0.5;
const MIN_SPLIT_RATIO: f32 = 0.2;
const MAX_SPLIT_RATIO: f32 = 0.8;
const MIN_PANE_RATIO: f32 = 0.08;
const RESIZE_HANDLE_SIZE: f32 = 6.0;
const TERMINAL_CHAR_WIDTH: f32 = 8.0;
const TERMINAL_LINE_HEIGHT: f32 = 18.0;
const TAB_HEIGHT: f32 = 54.0;
const API_BIND_ADDR: &str = "127.0.0.1:47431";
const API_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct LazytermApp {
    focus_handle: FocusHandle,
    cwd: PathBuf,
    branch: Option<String>,
    state_dir: PathBuf,
    sessions: Vec<TerminalSession>,
    active_session: usize,
    poller_started: bool,
    initial_focus_done: bool,
    keystroke_observer: Option<Subscription>,
    command_palette_open: bool,
    command_palette_query: String,
    command_palette_selection: usize,
    api_events: Receiver<ApiEvent>,
    ui_settings: UiSettings,
    resize_drag: Option<ResizeDrag>,
}

#[derive(Clone, Debug)]
struct UiSettings {
    tile_sessions: bool,
    tile_layout: TileLayout,
    rail_width: RailWidth,
    terminal_font_size: f32,
    terminal_padding: f32,
    split_ratio: f32,
    pane_ratios: Vec<f32>,
}

#[derive(Clone, Debug)]
struct ResizeDrag {
    orientation: SplitOrientation,
    handle_index: usize,
    start_position: Point<Pixels>,
    start_ratios: Vec<f32>,
}

#[derive(Clone)]
struct TerminalSizing {
    viewport: Size<Pixels>,
    font_size: f32,
    padding: f32,
    tile_layout: TileLayout,
    sidebar_width: f32,
    pane_ratios: Vec<f32>,
    session_count: usize,
    tiled: bool,
}

#[derive(Clone, Copy, Debug)]
struct TerminalContentBounds {
    origin: Point<Pixels>,
    columns: u16,
    rows: u16,
    cell_width: f32,
    line_height: f32,
}

#[derive(Clone, Copy, Debug)]
struct TerminalMouseInput {
    position: Point<Pixels>,
    button: Option<MouseButton>,
    modifiers: gpui::Modifiers,
    action: MouseReportAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AgentLaunchQuery {
    agent: AgentKind,
    cwd: PathBuf,
    task: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SplitOrientation {
    Columns,
    Rows,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
enum TileLayout {
    Grid,
    Columns,
    Rows,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
enum RailWidth {
    Compact,
    Default,
    Wide,
}

impl From<ApiTerminalRail> for RailWidth {
    fn from(value: ApiTerminalRail) -> Self {
        match value {
            ApiTerminalRail::Compact => Self::Compact,
            ApiTerminalRail::Default => Self::Default,
            ApiTerminalRail::Wide => Self::Wide,
        }
    }
}

impl From<ApiTileLayout> for TileLayout {
    fn from(value: ApiTileLayout) -> Self {
        match value {
            ApiTileLayout::Grid => Self::Grid,
            ApiTileLayout::Columns => Self::Columns,
            ApiTileLayout::Rows => Self::Rows,
        }
    }
}

impl From<PersistedUiSettings> for UiSettings {
    fn from(value: PersistedUiSettings) -> Self {
        Self {
            tile_sessions: value.tile_sessions,
            tile_layout: value.tile_layout,
            rail_width: value.rail_width,
            terminal_font_size: value.terminal_font_size.clamp(10.0, 16.0),
            terminal_padding: value.terminal_padding.clamp(8.0, 24.0),
            split_ratio: value.split_ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO),
            pane_ratios: value.pane_ratios,
        }
    }
}

impl From<UiSettings> for PersistedUiSettings {
    fn from(value: UiSettings) -> Self {
        Self {
            tile_sessions: value.tile_sessions,
            tile_layout: value.tile_layout,
            rail_width: value.rail_width,
            terminal_font_size: value.terminal_font_size,
            terminal_padding: value.terminal_padding,
            split_ratio: value.split_ratio,
            pane_ratios: value.pane_ratios,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedUiSettings {
    tile_sessions: bool,
    #[serde(default = "default_tile_layout")]
    tile_layout: TileLayout,
    #[serde(default = "default_rail_width")]
    rail_width: RailWidth,
    terminal_font_size: f32,
    #[serde(default = "default_terminal_padding")]
    terminal_padding: f32,
    #[serde(default = "default_split_ratio")]
    split_ratio: f32,
    #[serde(default)]
    pane_ratios: Vec<f32>,
}

struct TerminalSession {
    summary: SessionSummary,
    pty: Option<PtyHandle>,
    events: Receiver<PtyEvent>,
    terminal: TerminalGrid,
    lines: Vec<TerminalLine>,
    pending_line: String,
    pending_startup_input: Option<Vec<u8>>,
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

struct ApiEvent {
    request: ApiRequest,
    response: Sender<ApiResponse>,
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
enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseReportAction {
    Press,
    Release,
    Move,
    ScrollUp,
    ScrollDown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandKind {
    NewShell,
    NewCodex,
    NewClaude,
    NewOpenCode,
    NewGemini,
    NewAider,
    SplitPane,
    ToggleLayout,
    TileGrid,
    TileColumns,
    TileRows,
    RestartPane,
    ClosePane,
    CloseOtherPanes,
    FocusAttention,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    CopyTranscript,
    Paste,
    MaximizePane,
    DensityCompact,
    DensityDefault,
    DensityRoomy,
    RailCompact,
    RailDefault,
    RailWide,
    CompactFont,
    DefaultFont,
    FontDown,
    FontUp,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IconKind {
    NewPane,
    SplitLayout,
    CommandPalette,
    Close,
}

impl IconKind {
    fn asset_path(self) -> &'static str {
        match self {
            Self::NewPane => "icons/plus.svg",
            Self::SplitLayout => "icons/split.svg",
            Self::CommandPalette => "icons/command.svg",
            Self::Close => "icons/close.svg",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::NewPane => "new pane",
            Self::SplitLayout => "toggle split layout",
            Self::CommandPalette => "command palette",
            Self::Close => "close window",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandItem {
    kind: CommandKind,
    label: String,
    shortcut: String,
    meta: String,
}

impl CommandItem {
    fn new(
        kind: CommandKind,
        label: impl Into<String>,
        shortcut: impl Into<String>,
        meta: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            label: label.into(),
            shortcut: shortcut.into(),
            meta: meta.into(),
        }
    }
}

impl LazytermApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let branch = current_branch();
        let state_dir = app_state_dir();
        let (api_sender, api_events) = mpsc::channel();
        start_api_listener(api_sender);
        let ui_settings = load_ui_settings(&state_dir).unwrap_or(UiSettings {
            tile_sessions: false,
            tile_layout: TileLayout::Grid,
            rail_width: RailWidth::Compact,
            terminal_font_size: 12.0,
            terminal_padding: DEFAULT_TERMINAL_PADDING,
            split_ratio: DEFAULT_SPLIT_RATIO,
            pane_ratios: Vec::new(),
        });
        let sessions = load_session_summaries(&state_dir)
            .map(|summaries| spawn_persisted_sessions(summaries, branch.clone()))
            .filter(|sessions| !sessions.is_empty())
            .unwrap_or_else(|| {
                vec![TerminalSession::spawn(
                    1,
                    cwd.clone(),
                    branch.clone(),
                    "shell 1",
                    AgentKind::Shell,
                    None,
                )]
            });

        let mut app = Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            cwd,
            branch,
            state_dir,
            sessions,
            active_session: 0,
            poller_started: false,
            initial_focus_done: false,
            keystroke_observer: None,
            command_palette_open: false,
            command_palette_query: String::new(),
            command_palette_selection: 0,
            api_events,
            ui_settings,
            resize_drag: None,
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

        app.persist_state();
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
                        if app.poll_pty_events() || app.poll_api_events() {
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

    fn poll_api_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.api_events.try_recv() {
            let response = self.handle_api_request(event.request);
            changed |= !matches!(response, ApiResponse::Sessions(_));
            let _ = event.response.send(response);
        }
        changed
    }

    fn handle_api_request(&mut self, request: ApiRequest) -> ApiResponse {
        match request {
            ApiRequest::NewSession { cwd, agent, task } => {
                self.create_terminal_for(agent, cwd, task);
                ApiResponse::Ack
            }
            ApiRequest::ListSessions | ApiRequest::Status => ApiResponse::Sessions(
                self.sessions
                    .iter()
                    .map(|session| session.summary.clone())
                    .collect(),
            ),
            ApiRequest::FocusSession { id } => {
                let Some(index) = self
                    .sessions
                    .iter()
                    .position(|session| session.summary.id.as_str() == id)
                else {
                    return ApiResponse::Error {
                        message: format!("session '{id}' was not found"),
                    };
                };

                self.active_session = index;
                ApiResponse::Ack
            }
            ApiRequest::SendText { id, text, enter } => {
                let index = match id {
                    Some(id) => {
                        let Some(index) = self
                            .sessions
                            .iter()
                            .position(|session| session.summary.id.as_str() == id)
                        else {
                            return ApiResponse::Error {
                                message: format!("session '{id}' was not found"),
                            };
                        };
                        index
                    }
                    None => self.active_session,
                };
                self.write_text_to_session(index, &text, enter);
                ApiResponse::Ack
            }
            ApiRequest::RenameSession { id, title } => {
                let index = match id {
                    Some(id) => {
                        let Some(index) = self.session_index(&id) else {
                            return ApiResponse::Error {
                                message: format!("session '{id}' was not found"),
                            };
                        };
                        index
                    }
                    None => self.active_session,
                };
                let title = title.trim();
                if title.is_empty() {
                    return ApiResponse::Error {
                        message: "title cannot be empty".into(),
                    };
                }

                self.sessions[index].summary.title = title.to_string();
                self.sessions[index].summary.last_activity = "renamed".into();
                self.persist_state();
                ApiResponse::Ack
            }
            ApiRequest::CloseSession { id } => {
                if let Some(id) = id {
                    let Some(index) = self.session_index(&id) else {
                        return ApiResponse::Error {
                            message: format!("session '{id}' was not found"),
                        };
                    };
                    self.active_session = index;
                }
                self.close_active_terminal();
                ApiResponse::Ack
            }
            ApiRequest::RestartSession { id } => {
                if let Some(id) = id {
                    let Some(index) = self.session_index(&id) else {
                        return ApiResponse::Error {
                            message: format!("session '{id}' was not found"),
                        };
                    };
                    self.active_session = index;
                }
                self.restart_active_terminal();
                ApiResponse::Ack
            }
            ApiRequest::SplitWorkspace => {
                self.split_workspace();
                ApiResponse::Ack
            }
            ApiRequest::MaximizeSession => {
                self.maximize_active_terminal();
                ApiResponse::Ack
            }
            ApiRequest::CloseOtherSessions => {
                self.close_other_terminals();
                ApiResponse::Ack
            }
            ApiRequest::FocusAttention => {
                self.focus_next_attention_session();
                ApiResponse::Ack
            }
            ApiRequest::SetLayout { layout } => {
                self.set_tile_layout(TileLayout::from(layout));
                ApiResponse::Ack
            }
            ApiRequest::SetDensity { density } => {
                self.set_terminal_density(density);
                ApiResponse::Ack
            }
            ApiRequest::SetRail { rail } => {
                self.set_rail_width(RailWidth::from(rail));
                ApiResponse::Ack
            }
            ApiRequest::AgentHealth => ApiResponse::AgentHealth(agent_health_summaries()),
        }
    }

    fn resize_sessions(&mut self, viewport: Size<Pixels>) {
        let sizing = self.terminal_sizing(viewport);
        for (index, session) in self.sessions.iter_mut().enumerate() {
            let size = terminal_size_for_session(sizing.clone(), index);
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
        let primary = app_shortcut_modifiers(modifiers);

        if self.command_palette_open {
            match key {
                "escape" => {
                    self.command_palette_open = false;
                    self.command_palette_query.clear();
                    self.command_palette_selection = 0;
                    return true;
                }
                "backspace" => {
                    self.command_palette_query.pop();
                    self.command_palette_selection = 0;
                    return true;
                }
                "up" => {
                    self.select_previous_command();
                    return true;
                }
                "down" => {
                    self.select_next_command();
                    return true;
                }
                "enter" => {
                    if self.run_palette_launch_query() {
                        self.command_palette_open = false;
                        self.command_palette_query.clear();
                        self.command_palette_selection = 0;
                    } else if let Some(command) = self.selected_command() {
                        self.run_command(command.kind, cx);
                        self.command_palette_open = false;
                        self.command_palette_query.clear();
                        self.command_palette_selection = 0;
                    }
                    return true;
                }
                _ => {}
            }

            return modifiers.control || modifiers.alt || modifiers.platform || modifiers.function;
        }

        if modifiers.shift {
            match key {
                "pageup" => {
                    self.scroll_active_terminal(Scroll::PageUp);
                    return true;
                }
                "pagedown" => {
                    self.scroll_active_terminal(Scroll::PageDown);
                    return true;
                }
                "home" if modifiers.control => {
                    self.scroll_active_terminal(Scroll::Top);
                    return true;
                }
                "end" if modifiers.control => {
                    self.scroll_active_terminal(Scroll::Bottom);
                    return true;
                }
                _ => {}
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
                "u" => {
                    self.focus_next_attention_session();
                    return true;
                }
                "o" => {
                    self.close_other_terminals();
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
                "enter" => {
                    self.maximize_active_terminal();
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

        if modifiers.control && !modifiers.shift && !modifiers.alt && key == "v" {
            self.paste_clipboard(cx);
            return true;
        }

        if modifiers.control && key == "tab" {
            if modifiers.shift {
                self.activate_previous_session();
            } else {
                self.activate_next_session();
            }
            return true;
        }

        if modifiers.control && modifiers.alt {
            let direction = match key {
                "left" => Some(PaneDirection::Left),
                "right" => Some(PaneDirection::Right),
                "up" => Some(PaneDirection::Up),
                "down" => Some(PaneDirection::Down),
                _ => None,
            };
            if let Some(direction) = direction {
                self.activate_direction(direction);
                return true;
            }
        }

        false
    }

    fn write_keystroke_to_active_pty(&mut self, keystroke: &Keystroke) -> bool {
        let app_cursor = self.sessions[self.active_session].uses_app_cursor();
        let bytes = terminal_key_bytes(keystroke.key.as_str(), keystroke.modifiers, app_cursor);

        if let Some(bytes) = bytes {
            self.write_bytes_to_active_pty(&bytes);
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
            self.write_bytes_to_active_pty(input.as_bytes());
            return true;
        }

        false
    }

    fn write_bytes_to_active_pty(&mut self, bytes: &[u8]) {
        self.write_bytes_to_session(self.active_session, bytes);
    }

    fn scroll_active_terminal(&mut self, scroll: Scroll) {
        self.sessions[self.active_session].scroll_display(scroll);
    }

    fn scroll_terminal_from_wheel(
        &mut self,
        session_index: usize,
        event: &ScrollWheelEvent,
        viewport: Size<Pixels>,
    ) -> bool {
        let line_height = px(terminal_line_height(self.ui_settings.terminal_font_size));
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => delta.y.round() as i32,
            ScrollDelta::Pixels(delta) => (delta.y.as_f32() / line_height.as_f32()).round() as i32,
        };

        if lines == 0 {
            return false;
        }

        self.active_session = session_index;
        if self.report_terminal_mouse_scroll(
            session_index,
            event.position,
            event.modifiers,
            viewport,
            lines,
        ) {
            return true;
        }

        if self.sessions[session_index].uses_alternate_scroll() && !event.modifiers.shift {
            let bytes = alternate_scroll_bytes(lines);
            self.write_bytes_to_session(session_index, &bytes);
            return true;
        }

        self.sessions[session_index].scroll_display(Scroll::Delta(lines));
        true
    }

    fn report_terminal_mouse_input(
        &mut self,
        session_index: usize,
        input: TerminalMouseInput,
        viewport: Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.focus_terminal(window, cx);
        self.report_terminal_mouse(session_index, input, viewport)
    }

    fn report_terminal_mouse_scroll(
        &mut self,
        session_index: usize,
        position: Point<Pixels>,
        modifiers: gpui::Modifiers,
        viewport: Size<Pixels>,
        lines: i32,
    ) -> bool {
        if session_index >= self.sessions.len() {
            return false;
        }

        let mode = self.sessions[session_index].mouse_mode();
        if !terminal_mouse_mode_enabled(mode) {
            return false;
        }

        let sizing = self.terminal_sizing(viewport);
        let bounds = terminal_content_bounds_for_session(sizing, session_index);
        let Some((column, row)) = terminal_point_for_mouse(position, bounds) else {
            return false;
        };
        let Some(bytes) = terminal_mouse_scroll_report_bytes(column, row, lines, modifiers, mode)
        else {
            return false;
        };

        self.write_bytes_to_session(session_index, &bytes);
        true
    }

    fn report_terminal_mouse(
        &mut self,
        session_index: usize,
        input: TerminalMouseInput,
        viewport: Size<Pixels>,
    ) -> bool {
        if session_index >= self.sessions.len() {
            return false;
        }

        let mode = self.sessions[session_index].mouse_mode();
        if !terminal_mouse_mode_enabled(mode) {
            return false;
        }

        if input.action == MouseReportAction::Move
            && !terminal_mouse_move_enabled(mode, input.button.is_some())
        {
            return false;
        }

        let sizing = self.terminal_sizing(viewport);
        let bounds = terminal_content_bounds_for_session(sizing, session_index);
        let Some((column, row)) = terminal_point_for_mouse(input.position, bounds) else {
            return false;
        };

        let Some(bytes) = terminal_mouse_report_bytes(
            column,
            row,
            input.button,
            input.action,
            input.modifiers,
            mode,
        ) else {
            return false;
        };

        self.active_session = session_index;
        self.write_bytes_to_session(session_index, &bytes);
        true
    }

    fn terminal_sizing(&self, viewport: Size<Pixels>) -> TerminalSizing {
        TerminalSizing {
            viewport,
            font_size: self.ui_settings.terminal_font_size,
            padding: self.ui_settings.terminal_padding,
            tile_layout: self.ui_settings.tile_layout,
            sidebar_width: self.sidebar_width(),
            pane_ratios: self.ui_settings.pane_ratios.clone(),
            session_count: self.sessions.len(),
            tiled: self.ui_settings.tile_sessions,
        }
    }

    fn write_text_to_session(&mut self, session_index: usize, text: &str, enter: bool) {
        let mut bytes = text.as_bytes().to_vec();
        if enter {
            bytes.push(b'\r');
        }
        self.write_bytes_to_session(session_index, &bytes);
    }

    fn write_bytes_to_session(&mut self, session_index: usize, bytes: &[u8]) {
        let session = &mut self.sessions[session_index];
        let Some(pty) = &mut session.pty else {
            return;
        };

        if let Err(error) = pty.write_all(bytes) {
            session
                .lines
                .push(TerminalLine::error(format!("write failed: {error}")));
            session.summary.status = SessionStatus::Failed;
            session.summary.notification = Some("write failed".into());
        } else {
            session.summary.status = SessionStatus::Running;
            session.summary.notification = None;
            session.summary.last_activity = "input sent".into();
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
            None,
        ));
        self.active_session = self.sessions.len() - 1;
        self.persist_state();
    }

    fn create_agent_terminal(&mut self, agent: AgentKind) {
        self.create_terminal_for(agent, self.cwd.clone(), None);
    }

    fn create_terminal_for(&mut self, agent: AgentKind, cwd: PathBuf, task: Option<String>) {
        let index = self.sessions.len() + 1;
        let title = match agent {
            AgentKind::Shell => format!("shell {index}"),
            _ => format!("{} {index}", agent.label().to_ascii_lowercase()),
        };
        self.sessions.push(TerminalSession::spawn(
            index,
            cwd.clone(),
            current_branch_for(&cwd),
            title,
            agent,
            task.map(startup_input_bytes),
        ));
        self.active_session = self.sessions.len() - 1;

        self.persist_state();
    }

    fn run_palette_launch_query(&mut self) -> bool {
        let Some(launch) = parse_agent_launch_query(&self.command_palette_query, &self.cwd) else {
            return false;
        };

        self.create_terminal_for(launch.agent, launch.cwd, launch.task);
        true
    }

    fn close_active_terminal(&mut self) {
        if self.sessions.len() == 1 {
            return;
        }

        self.sessions.remove(self.active_session);
        if self.active_session >= self.sessions.len() {
            self.active_session = self.sessions.len() - 1;
        }
        self.persist_state();
    }

    fn close_other_terminals(&mut self) {
        if self.sessions.len() <= 1 {
            return;
        }

        let active = self.sessions.remove(self.active_session);
        self.sessions.clear();
        self.sessions.push(active);
        self.active_session = 0;
        self.persist_state();
    }

    fn restart_active_terminal(&mut self) {
        let index = self.active_session + 1;
        let title = self.sessions[self.active_session].summary.title.clone();
        let agent = self.sessions[self.active_session].summary.agent;
        self.sessions[self.active_session] = TerminalSession::spawn(
            index,
            self.cwd.clone(),
            self.branch.clone(),
            title,
            agent,
            None,
        );
        self.persist_state();
    }

    fn maximize_active_terminal(&mut self) {
        self.ui_settings.tile_sessions = false;
        self.persist_ui_settings();
    }

    fn focus_next_attention_session(&mut self) {
        if self.sessions.is_empty() {
            return;
        }

        for offset in 1..=self.sessions.len() {
            let index = (self.active_session + offset) % self.sessions.len();
            if session_needs_attention(&self.sessions[index]) {
                self.active_session = index;
                return;
            }
        }
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

    fn activate_direction(&mut self, direction: PaneDirection) {
        if self.sessions.len() <= 1 {
            return;
        }

        if !self.ui_settings.tile_sessions {
            match direction {
                PaneDirection::Left | PaneDirection::Up => self.activate_previous_session(),
                PaneDirection::Right | PaneDirection::Down => self.activate_next_session(),
            }
            return;
        }

        let columns = tile_columns_for_layout(self.sessions.len(), self.ui_settings.tile_layout);
        if let Some(index) =
            directional_session_index(self.active_session, self.sessions.len(), columns, direction)
        {
            self.active_session = index;
        }
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };

        let bracketed = self.sessions[self.active_session].uses_bracketed_paste();
        let bytes = paste_bytes_for_terminal(&text, bracketed);
        self.write_bytes_to_active_pty(&bytes);
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
        self.persist_ui_settings();
    }

    fn set_font_size(&mut self, size: f32) {
        self.ui_settings.terminal_font_size = size.clamp(10.0, 16.0);
        self.persist_ui_settings();
    }

    fn set_density(&mut self, font_size: f32, padding: f32) {
        self.ui_settings.terminal_font_size = font_size.clamp(10.0, 16.0);
        self.ui_settings.terminal_padding = padding.clamp(8.0, 24.0);
        self.persist_ui_settings();
    }

    fn set_terminal_density(&mut self, density: ApiTerminalDensity) {
        match density {
            ApiTerminalDensity::Compact => self.set_density(11.0, 10.0),
            ApiTerminalDensity::Default => self.set_density(12.0, DEFAULT_TERMINAL_PADDING),
            ApiTerminalDensity::Roomy => self.set_density(13.0, 22.0),
        }
    }

    fn set_rail_width(&mut self, width: RailWidth) {
        self.ui_settings.rail_width = width;
        self.persist_ui_settings();
    }

    fn sidebar_width(&self) -> f32 {
        sidebar_width_for_rail(self.ui_settings.rail_width)
    }

    fn show_rail_metadata(&self) -> bool {
        self.ui_settings.rail_width == RailWidth::Wide
    }

    fn toggle_tile_sessions(&mut self) {
        self.ui_settings.tile_sessions = !self.ui_settings.tile_sessions;
        self.persist_ui_settings();
    }

    fn set_tile_layout(&mut self, layout: TileLayout) {
        self.ui_settings.tile_layout = layout;
        self.ui_settings.tile_sessions = true;
        self.persist_ui_settings();
    }

    fn start_resize_drag(
        &mut self,
        orientation: SplitOrientation,
        handle_index: usize,
        position: Point<Pixels>,
    ) {
        self.resize_drag = Some(ResizeDrag {
            orientation,
            handle_index,
            start_position: position,
            start_ratios: pane_ratios_for_count(&self.ui_settings, self.sessions.len()),
        });
    }

    fn update_resize_drag(&mut self, position: Point<Pixels>, viewport: Size<Pixels>) -> bool {
        let Some(drag) = self.resize_drag.as_ref() else {
            return false;
        };

        let pane_count = self.sessions.len();
        if pane_count < 2 || drag.handle_index + 1 >= pane_count {
            return false;
        }

        let handle_space = RESIZE_HANDLE_SIZE * (pane_count.saturating_sub(1) as f32);
        let available = match drag.orientation {
            SplitOrientation::Columns => {
                (viewport.width.as_f32() - self.sidebar_width() - handle_space).max(1.0)
            }
            SplitOrientation::Rows => {
                (viewport.height.as_f32() - TITLEBAR_HEIGHT - handle_space).max(1.0)
            }
        };
        let delta = match drag.orientation {
            SplitOrientation::Columns => position.x.as_f32() - drag.start_position.x.as_f32(),
            SplitOrientation::Rows => position.y.as_f32() - drag.start_position.y.as_f32(),
        };
        let mut ratios = normalize_pane_ratios(&drag.start_ratios, pane_count);
        let left = drag.handle_index;
        let right = left + 1;
        let pair_total = ratios[left] + ratios[right];
        let min_ratio = MIN_PANE_RATIO.min(pair_total / 2.0);
        let next_left =
            (ratios[left] + (delta / available)).clamp(min_ratio, pair_total - min_ratio);
        ratios[left] = next_left;
        ratios[right] = pair_total - next_left;
        self.ui_settings.pane_ratios = normalize_pane_ratios(&ratios, pane_count);
        if pane_count == 2 {
            self.ui_settings.split_ratio =
                self.ui_settings.pane_ratios[0].clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
        }
        true
    }

    fn stop_resize_drag(&mut self) -> bool {
        if self.resize_drag.take().is_some() {
            self.persist_ui_settings();
            return true;
        }
        false
    }

    fn split_workspace(&mut self) {
        if self.sessions.len() == 1 {
            self.create_terminal();
        }
        self.ui_settings.tile_sessions = true;
        self.persist_ui_settings();
    }

    fn run_command(&mut self, command: CommandKind, cx: &mut Context<Self>) {
        match command {
            CommandKind::NewShell => self.create_terminal(),
            CommandKind::NewCodex => self.create_agent_terminal(AgentKind::Codex),
            CommandKind::NewClaude => self.create_agent_terminal(AgentKind::Claude),
            CommandKind::NewOpenCode => self.create_agent_terminal(AgentKind::OpenCode),
            CommandKind::NewGemini => self.create_agent_terminal(AgentKind::Gemini),
            CommandKind::NewAider => self.create_agent_terminal(AgentKind::Aider),
            CommandKind::SplitPane => self.split_workspace(),
            CommandKind::ToggleLayout => self.toggle_tile_sessions(),
            CommandKind::TileGrid => self.set_tile_layout(TileLayout::Grid),
            CommandKind::TileColumns => self.set_tile_layout(TileLayout::Columns),
            CommandKind::TileRows => self.set_tile_layout(TileLayout::Rows),
            CommandKind::RestartPane => self.restart_active_terminal(),
            CommandKind::ClosePane => self.close_active_terminal(),
            CommandKind::CloseOtherPanes => self.close_other_terminals(),
            CommandKind::FocusAttention => self.focus_next_attention_session(),
            CommandKind::FocusLeft => self.activate_direction(PaneDirection::Left),
            CommandKind::FocusRight => self.activate_direction(PaneDirection::Right),
            CommandKind::FocusUp => self.activate_direction(PaneDirection::Up),
            CommandKind::FocusDown => self.activate_direction(PaneDirection::Down),
            CommandKind::CopyTranscript => self.copy_active_transcript(cx),
            CommandKind::Paste => self.paste_clipboard(cx),
            CommandKind::MaximizePane => self.maximize_active_terminal(),
            CommandKind::DensityCompact => self.set_density(11.0, 10.0),
            CommandKind::DensityDefault => self.set_density(12.0, DEFAULT_TERMINAL_PADDING),
            CommandKind::DensityRoomy => self.set_density(13.0, 22.0),
            CommandKind::RailCompact => self.set_rail_width(RailWidth::Compact),
            CommandKind::RailDefault => self.set_rail_width(RailWidth::Default),
            CommandKind::RailWide => self.set_rail_width(RailWidth::Wide),
            CommandKind::CompactFont => self.set_font_size(11.0),
            CommandKind::DefaultFont => self.set_font_size(12.0),
            CommandKind::FontDown => self.adjust_font_size(-1.0),
            CommandKind::FontUp => self.adjust_font_size(1.0),
            CommandKind::ScrollPageUp => self.scroll_active_terminal(Scroll::PageUp),
            CommandKind::ScrollPageDown => self.scroll_active_terminal(Scroll::PageDown),
            CommandKind::ScrollTop => self.scroll_active_terminal(Scroll::Top),
            CommandKind::ScrollBottom => self.scroll_active_terminal(Scroll::Bottom),
        }
    }

    fn commands(&self) -> Vec<CommandItem> {
        let layout_label = if self.ui_settings.tile_sessions {
            "single pane"
        } else {
            "tile panes"
        };

        let agent_status = |agent| agent_health_label(agent).to_string();

        vec![
            CommandItem::new(CommandKind::NewShell, "new shell", "ctrl+shift+t", ""),
            CommandItem::new(
                CommandKind::NewCodex,
                "new codex",
                "",
                agent_status(AgentKind::Codex),
            ),
            CommandItem::new(
                CommandKind::NewClaude,
                "new claude",
                "",
                agent_status(AgentKind::Claude),
            ),
            CommandItem::new(
                CommandKind::NewOpenCode,
                "new opencode",
                "",
                agent_status(AgentKind::OpenCode),
            ),
            CommandItem::new(
                CommandKind::NewGemini,
                "new gemini",
                "",
                agent_status(AgentKind::Gemini),
            ),
            CommandItem::new(
                CommandKind::NewAider,
                "new aider",
                "",
                agent_status(AgentKind::Aider),
            ),
            CommandItem::new(CommandKind::SplitPane, "split pane", "ctrl+shift+b", ""),
            CommandItem::new(CommandKind::ToggleLayout, layout_label, "", ""),
            CommandItem::new(CommandKind::TileGrid, "grid", "", ""),
            CommandItem::new(CommandKind::TileColumns, "columns", "", ""),
            CommandItem::new(CommandKind::TileRows, "rows", "", ""),
            CommandItem::new(
                CommandKind::MaximizePane,
                "maximize pane",
                "ctrl+shift+enter",
                "",
            ),
            CommandItem::new(
                CommandKind::FocusAttention,
                "focus attention",
                "ctrl+shift+u",
                "",
            ),
            CommandItem::new(CommandKind::FocusLeft, "focus left", "ctrl+alt+left", ""),
            CommandItem::new(CommandKind::FocusRight, "focus right", "ctrl+alt+right", ""),
            CommandItem::new(CommandKind::FocusUp, "focus up", "ctrl+alt+up", ""),
            CommandItem::new(CommandKind::FocusDown, "focus down", "ctrl+alt+down", ""),
            CommandItem::new(CommandKind::RestartPane, "restart pane", "ctrl+shift+r", ""),
            CommandItem::new(CommandKind::ClosePane, "close pane", "ctrl+shift+w", ""),
            CommandItem::new(
                CommandKind::CloseOtherPanes,
                "close other panes",
                "ctrl+shift+o",
                "",
            ),
            CommandItem::new(
                CommandKind::CopyTranscript,
                "copy transcript",
                "ctrl+shift+c",
                "",
            ),
            CommandItem::new(CommandKind::Paste, "paste", "ctrl+shift+v", ""),
            CommandItem::new(
                CommandKind::ScrollPageUp,
                "scroll page up",
                "shift+pageup",
                "",
            ),
            CommandItem::new(
                CommandKind::ScrollPageDown,
                "scroll page down",
                "shift+pagedown",
                "",
            ),
            CommandItem::new(CommandKind::ScrollTop, "scroll top", "ctrl+shift+home", ""),
            CommandItem::new(
                CommandKind::ScrollBottom,
                "scroll bottom",
                "ctrl+shift+end",
                "",
            ),
            CommandItem::new(CommandKind::DensityCompact, "compact density", "", ""),
            CommandItem::new(CommandKind::DensityDefault, "default density", "", ""),
            CommandItem::new(CommandKind::DensityRoomy, "roomy density", "", ""),
            CommandItem::new(CommandKind::RailCompact, "compact rail", "", ""),
            CommandItem::new(CommandKind::RailDefault, "default rail", "", ""),
            CommandItem::new(CommandKind::RailWide, "wide rail", "", ""),
            CommandItem::new(CommandKind::CompactFont, "compact font", "", ""),
            CommandItem::new(CommandKind::DefaultFont, "default font", "", ""),
            CommandItem::new(CommandKind::FontDown, "smaller font", "ctrl+shift+-", ""),
            CommandItem::new(CommandKind::FontUp, "larger font", "ctrl+shift+=", ""),
        ]
    }

    fn filtered_commands(&self) -> Vec<CommandItem> {
        let query = self.command_palette_query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return self.commands();
        }

        self.commands()
            .into_iter()
            .filter(|command| {
                command.label.contains(&query)
                    || command.shortcut.contains(&query)
                    || command.meta.contains(&query)
            })
            .collect()
    }

    fn selected_command(&self) -> Option<CommandItem> {
        let commands = self.filtered_commands();
        if commands.is_empty() {
            return None;
        }

        commands
            .get(self.command_palette_selection.min(commands.len() - 1))
            .cloned()
    }

    fn select_previous_command(&mut self) {
        let command_count = self.filtered_commands().len();
        if command_count == 0 {
            self.command_palette_selection = 0;
            return;
        }

        self.command_palette_selection = if self.command_palette_selection == 0 {
            command_count - 1
        } else {
            self.command_palette_selection - 1
        };
    }

    fn select_next_command(&mut self) {
        let command_count = self.filtered_commands().len();
        if command_count == 0 {
            self.command_palette_selection = 0;
            return;
        }

        self.command_palette_selection = (self.command_palette_selection + 1) % command_count;
    }

    fn focus_terminal(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    fn active_session(&self) -> &TerminalSession {
        &self.sessions[self.active_session]
    }

    fn session_index(&self, id: &str) -> Option<usize> {
        self.sessions
            .iter()
            .position(|session| session.summary.id.as_str() == id)
    }

    fn toggle_command_palette(&mut self) {
        self.command_palette_open = !self.command_palette_open;
        if !self.command_palette_open {
            self.command_palette_query.clear();
            self.command_palette_selection = 0;
        }
    }

    fn push_command_palette_text(&mut self, text: &str) {
        for character in text.chars() {
            match character {
                '\u{8}' | '\u{7f}' => {
                    self.command_palette_query.pop();
                    self.command_palette_selection = 0;
                }
                '\r' | '\n' => {}
                character if !character.is_control() => {
                    self.command_palette_query.push(character);
                    self.command_palette_selection = 0;
                }
                _ => {}
            }
        }
    }

    fn persist_state(&self) {
        self.persist_ui_settings();
        self.persist_sessions();
    }

    fn persist_ui_settings(&self) {
        if let Err(error) = save_ui_settings(&self.state_dir, self.ui_settings.clone()) {
            eprintln!("lazyterm: failed to save ui settings: {error}");
        }
    }

    fn persist_sessions(&self) {
        if let Err(error) = save_session_summaries(
            &self.state_dir,
            &self
                .sessions
                .iter()
                .map(|session| session.summary.clone())
                .collect::<Vec<_>>(),
        ) {
            eprintln!("lazyterm: failed to save sessions: {error}");
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
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .font_family("JetBrains Mono")
                    .child(
                        div()
                            .size(px(24.0))
                            .rounded(px(4.0))
                            .overflow_hidden()
                            .child(img("logoblackbackground.svg").size_full()),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(TEXT))
                            .child("lazyterm"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(self.render_titlebar_button(
                        IconKind::NewPane,
                        "titlebar-new",
                        cx,
                        |this, _| {
                            this.create_terminal();
                        },
                    ))
                    .child(self.render_titlebar_button(
                        IconKind::SplitLayout,
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
                    .child(self.render_titlebar_button(
                        IconKind::CommandPalette,
                        "titlebar-command",
                        cx,
                        |this, _| {
                            this.toggle_command_palette();
                        },
                    ))
                    .child(self.render_titlebar_button(
                        IconKind::Close,
                        "window-close",
                        cx,
                        |_, window| {
                            window.remove_window();
                        },
                    )),
            )
    }

    fn render_titlebar_button(
        &self,
        icon: IconKind,
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
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
            .when(icon == IconKind::Close, |this| {
                this.window_control_area(WindowControlArea::Close)
            })
            .hover(|this| this.bg(rgb(ROW_ACTIVE)).border_color(rgb(TEXT_DIM)))
            .child(
                img(icon.asset_path())
                    .w(px(14.0))
                    .h(px(14.0))
                    .id(format!("{}-icon", icon.label())),
            )
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
            .gap_1()
            .w(px(self.sidebar_width()))
            .h_full()
            .border_r_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SIDEBAR))
            .px_1()
            .py_1()
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .py_1()
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
            SessionStatus::NeedsInput => TEXT,
            SessionStatus::Waiting => TEXT_MUTED,
            SessionStatus::Done => TEXT_DIM,
            _ => TEXT_SOFT,
        };
        let attention = session_needs_attention(session);
        let tab_background = if active { SURFACE_ACTIVE } else { SIDEBAR };
        let show_metadata = self.show_rail_metadata();
        let compact = self.ui_settings.rail_width == RailWidth::Compact;

        let mut tab_body = div()
            .flex()
            .items_center()
            .gap_2()
            .pl_3()
            .pr_2()
            .w_full()
            .child(
                div()
                    .w(px(if compact { 32.0 } else { 28.0 }))
                    .text_color(rgb(if active { TEXT } else { TEXT_MUTED }))
                    .text_size(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .child(format!("{:02}", index + 1)),
            );

        if compact {
            tab_body = tab_body.justify_center();
        } else {
            tab_body = tab_body.child(
                div()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_color(rgb(if active { TEXT } else { TEXT_SOFT }))
                            .text_size(px(13.0))
                            .font_weight(if active {
                                FontWeight::BOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(SharedString::from(session.summary.title.clone())),
                    )
                    .when(show_metadata, |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_color(rgb(if active { TEXT_MUTED } else { TEXT_DIM }))
                                .text_size(px(10.0))
                                .child(SharedString::from(status_label(session.summary.status)))
                                .child(SharedString::from(" / "))
                                .child(SharedString::from(session.summary.agent.label())),
                        )
                    }),
            );
        }

        div()
            .flex()
            .items_center()
            .relative()
            .w_full()
            .h(px(TAB_HEIGHT))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(if active || attention {
                BORDER_ACTIVE
            } else {
                BORDER
            }))
            .bg(rgb(tab_background))
            .hover(|this| this.bg(rgb(SURFACE)).border_color(rgb(TEXT_DIM)))
            .font_family("JetBrains Mono")
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(7.0))
                    .bottom(px(7.0))
                    .w(px(2.0))
                    .rounded_full()
                    .bg(rgb(if active { TEXT } else { status_color })),
            )
            .when(attention, |this| {
                this.child(
                    div()
                        .absolute()
                        .right(px(6.0))
                        .top(px(5.0))
                        .text_color(rgb(TEXT))
                        .text_size(px(10.0))
                        .child("!"),
                )
            })
            .child(tab_body)
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
        match self.ui_settings.tile_layout {
            TileLayout::Columns if self.sessions.len() >= 2 => {
                return self.render_linear_split(SplitOrientation::Columns, cx);
            }
            TileLayout::Rows if self.sessions.len() >= 2 => {
                return self.render_linear_split(SplitOrientation::Rows, cx);
            }
            TileLayout::Grid | TileLayout::Columns | TileLayout::Rows => {}
        }

        let columns = tile_columns_for_layout(self.sessions.len(), self.ui_settings.tile_layout);
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

    fn render_linear_split(&self, orientation: SplitOrientation, cx: &mut Context<Self>) -> Div {
        let cursor = match orientation {
            SplitOrientation::Columns => CursorStyle::ResizeLeftRight,
            SplitOrientation::Rows => CursorStyle::ResizeUpDown,
        };
        let ratios = pane_ratios_for_count(&self.ui_settings, self.sessions.len());
        let mut split = div()
            .flex()
            .when(orientation == SplitOrientation::Columns, |this| {
                this.flex_row()
            })
            .when(orientation == SplitOrientation::Rows, |this| {
                this.flex_col()
            })
            .flex_1()
            .h_full()
            .bg(rgb(BG));

        for (index, ratio) in ratios.iter().copied().enumerate().take(self.sessions.len()) {
            split = split.child(
                div()
                    .flex()
                    .flex_col()
                    .h_full()
                    .flex_basis(relative(ratio))
                    .overflow_hidden()
                    .child(self.render_terminal_tile(index, cx)),
            );

            if index + 1 < self.sessions.len() {
                split = split.child(self.render_resize_handle(orientation, index, cursor, cx));
            }
        }

        split
    }

    fn render_resize_handle(
        &self,
        orientation: SplitOrientation,
        handle_index: usize,
        cursor: CursorStyle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex_none()
            .when(orientation == SplitOrientation::Columns, |this| {
                this.w(px(RESIZE_HANDLE_SIZE)).h_full()
            })
            .when(orientation == SplitOrientation::Rows, |this| {
                this.h(px(RESIZE_HANDLE_SIZE)).w_full()
            })
            .bg(rgb(BORDER))
            .hover(|this| this.bg(rgb(TEXT_FAINT)))
            .cursor(cursor)
            .id(format!("pane-resize-handle-{handle_index}"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.start_resize_drag(orientation, handle_index, event.position);
                    this.focus_terminal(window, cx);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.stop_resize_drag() {
                        cx.notify();
                    }
                }),
            )
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
        cx: &mut Context<Self>,
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
            .child(
                div()
                    .flex_1()
                    .px(px(self.ui_settings.terminal_padding))
                    .py(px(self.ui_settings.terminal_padding))
                    .font_family("JetBrains Mono")
                    .text_size(px(self.ui_settings.terminal_font_size))
                    .line_height(px(terminal_line_height(
                        self.ui_settings.terminal_font_size,
                    )))
                    .id("terminal-transcript")
                    .overflow_y_scroll()
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &ScrollWheelEvent, window, cx| {
                            if this.scroll_terminal_from_wheel(
                                session_index,
                                event,
                                window.viewport_size(),
                            ) {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        },
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            if this.report_terminal_mouse_input(
                                session_index,
                                TerminalMouseInput {
                                    position: event.position,
                                    button: Some(MouseButton::Left),
                                    modifiers: event.modifiers,
                                    action: MouseReportAction::Press,
                                },
                                window.viewport_size(),
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            if this.report_terminal_mouse_input(
                                session_index,
                                TerminalMouseInput {
                                    position: event.position,
                                    button: Some(MouseButton::Middle),
                                    modifiers: event.modifiers,
                                    action: MouseReportAction::Press,
                                },
                                window.viewport_size(),
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            if this.report_terminal_mouse_input(
                                session_index,
                                TerminalMouseInput {
                                    position: event.position,
                                    button: Some(MouseButton::Right),
                                    modifiers: event.modifiers,
                                    action: MouseReportAction::Press,
                                },
                                window.viewport_size(),
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            if this.report_terminal_mouse_input(
                                session_index,
                                TerminalMouseInput {
                                    position: event.position,
                                    button: Some(MouseButton::Left),
                                    modifiers: event.modifiers,
                                    action: MouseReportAction::Release,
                                },
                                window.viewport_size(),
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            if this.report_terminal_mouse_input(
                                session_index,
                                TerminalMouseInput {
                                    position: event.position,
                                    button: Some(MouseButton::Middle),
                                    modifiers: event.modifiers,
                                    action: MouseReportAction::Release,
                                },
                                window.viewport_size(),
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            if this.report_terminal_mouse_input(
                                session_index,
                                TerminalMouseInput {
                                    position: event.position,
                                    button: Some(MouseButton::Right),
                                    modifiers: event.modifiers,
                                    action: MouseReportAction::Release,
                                },
                                window.viewport_size(),
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_move(
                        cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                            if this.report_terminal_mouse_input(
                                session_index,
                                TerminalMouseInput {
                                    position: event.position,
                                    button: event.pressed_button,
                                    modifiers: event.modifiers,
                                    action: MouseReportAction::Move,
                                },
                                window.viewport_size(),
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                    )
                    .child(terminal_grid),
            )
            .child(self.render_statusline(session_index, focused))
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
        let notification = session.summary.notification.as_deref().unwrap_or("");
        let context = session_context_label(session);
        let status = status_label(session.summary.status);
        let detail = if notification.is_empty() {
            status.to_string()
        } else {
            notification.to_string()
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(STATUSLINE_HEIGHT))
            .border_t_1()
            .border_color(rgb(if focused { BORDER_ACTIVE } else { BORDER }))
            .bg(rgb(BG))
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
                    .child(SharedString::from(context)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(rgb(TEXT_DIM))
                    .child(SharedString::from(detail)),
            )
    }

    fn render_command_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filtered_commands = self.filtered_commands();
        let has_commands = !filtered_commands.is_empty();
        let mut commands = div().flex().flex_col().gap_1();
        for (index, command) in filtered_commands.into_iter().enumerate() {
            commands = commands.child(self.render_command_action(
                command,
                index == self.command_palette_selection,
                cx,
            ));
        }

        if !has_commands {
            commands = commands.child(
                div()
                    .min_h(px(34.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_color(rgb(TEXT_DIM))
                    .text_size(px(12.0))
                    .child("no matches"),
            );
        }

        div()
            .flex()
            .flex_col()
            .w(px(COMMAND_PALETTE_WIDTH))
            .max_h(px(COMMAND_PALETTE_MAX_HEIGHT))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(BORDER_ACTIVE))
            .bg(rgb(BG))
            .font_family("JetBrains Mono")
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .child(self.render_command_query())
                    .child(commands.id("command-list").overflow_y_scroll()),
            )
    }

    fn render_command_action(
        &self,
        command: CommandItem,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let detail = command_detail(&command);

        div()
            .flex()
            .items_center()
            .justify_between()
            .min_h(px(36.0))
            .px_3()
            .rounded(px(4.0))
            .bg(rgb(if selected { ROW_ACTIVE } else { BG }))
            .border_1()
            .border_color(rgb(if selected { TEXT_SOFT } else { BORDER }))
            .hover(|this| this.bg(rgb(ROW_ACTIVE)).border_color(rgb(TEXT_DIM)))
            .child(
                div()
                    .text_color(rgb(if selected { TEXT } else { TEXT_SOFT }))
                    .text_size(px(12.0))
                    .child(SharedString::from(command.label.clone())),
            )
            .when(!detail.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(TEXT_DIM))
                        .text_size(px(11.0))
                        .child(SharedString::from(detail.clone())),
                )
            })
            .id(format!("command-{:?}", command.kind))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.run_command(command.kind, cx);
                this.command_palette_open = false;
                this.command_palette_query.clear();
                this.command_palette_selection = 0;
                this.focus_terminal(window, cx);
                cx.notify();
            }))
    }

    fn render_command_query(&self) -> impl IntoElement {
        let text = if self.command_palette_query.is_empty() {
            ">".into()
        } else {
            format!("> {}", self.command_palette_query)
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .min_h(px(36.0))
            .px_3()
            .rounded(px(4.0))
            .bg(rgb(SURFACE))
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
    }
}

impl TerminalSession {
    fn spawn(
        index: usize,
        cwd: PathBuf,
        branch: Option<String>,
        title: impl Into<String>,
        agent: AgentKind,
        pending_startup_input: Option<Vec<u8>>,
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
            pending_startup_input,
            terminal_size: TerminalSize::DEFAULT,
        }
    }

    fn push_output(&mut self, output: &[u8]) {
        self.terminal.feed(output);
        self.flush_terminal_replies();
        self.flush_startup_input();
        let output = normalize_pty_output(&String::from_utf8_lossy(output));
        self.update_activity_from_output(&output);
        for segment in output.split_inclusive('\n') {
            if segment.ends_with('\n') {
                self.pending_line.push_str(segment.trim_end_matches('\n'));
                self.flush_pending_line();
            } else {
                self.pending_line.push_str(segment);
            }
        }
    }

    fn update_activity_from_output(&mut self, output: &str) {
        if output.trim().is_empty() {
            return;
        }

        let status = detect_status(output);
        self.summary.status = status;
        self.summary.last_activity = match status {
            SessionStatus::Running => "output".into(),
            SessionStatus::Waiting => "waiting".into(),
            SessionStatus::NeedsInput => "needs input".into(),
            SessionStatus::Failed => "failed".into(),
            SessionStatus::Done => "done".into(),
        };
        self.summary.notification = notification_for_status(status, output);
    }

    fn flush_startup_input(&mut self) {
        let Some(input) = self.pending_startup_input.take() else {
            return;
        };
        let Some(pty) = &mut self.pty else {
            return;
        };

        if let Err(error) = pty.write_all(&input) {
            self.lines.push(TerminalLine::error(format!(
                "startup input failed: {error}"
            )));
            self.summary.status = SessionStatus::Failed;
        }
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

    fn scroll_display(&mut self, scroll: Scroll) {
        self.terminal.scroll_display(scroll);
    }

    fn uses_alternate_scroll(&self) -> bool {
        self.terminal.uses_alternate_scroll()
    }

    fn uses_bracketed_paste(&self) -> bool {
        self.terminal.uses_bracketed_paste()
    }

    fn uses_app_cursor(&self) -> bool {
        self.terminal.uses_app_cursor()
    }

    fn mouse_mode(&self) -> TermMode {
        self.terminal.mode()
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

    fn scroll_display(&mut self, scroll: Scroll) {
        self.term.scroll_display(scroll);
    }

    fn uses_alternate_scroll(&self) -> bool {
        self.term
            .mode()
            .contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
    }

    fn uses_bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    fn uses_app_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    #[cfg(test)]
    fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
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
        style.size.width = px(0.0).into();
        style.size.height = px(0.0).into();
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
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.update_resize_drag(event.position, window.viewport_size()) {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if this.stop_resize_drag() {
                        cx.notify();
                    }
                }),
            )
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
                                .top(px(COMMAND_PALETTE_TOP))
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
    terminal_padding: f32,
    sidebar_width: f32,
    tile_layout: TileLayout,
    session_count: usize,
    tiled: bool,
) -> TerminalSize {
    let width = viewport.width.as_f32();
    let height = viewport.height.as_f32();
    let panes = if tiled { session_count.max(1) } else { 1 };
    let tile_columns = tile_columns_for_layout(panes, tile_layout) as f32;
    let tile_rows = ((panes as f32) / tile_columns).ceil().max(1.0);
    let terminal_width =
        ((width - sidebar_width) / tile_columns - (terminal_padding * 2.0)).max(160.0);
    let terminal_height =
        ((height - TITLEBAR_HEIGHT) / tile_rows - STATUSLINE_HEIGHT - (terminal_padding * 2.0))
            .max(96.0);
    let columns = (terminal_width / terminal_char_width(terminal_font_size))
        .floor()
        .max(20.0) as u16;
    let rows = (terminal_height / terminal_line_height(terminal_font_size))
        .floor()
        .max(5.0) as u16;

    TerminalSize::new(columns, rows)
}

fn terminal_size_for_session(sizing: TerminalSizing, session_index: usize) -> TerminalSize {
    if sizing.tiled && sizing.session_count >= 2 {
        match sizing.tile_layout {
            TileLayout::Columns => {
                let ratios = normalize_pane_ratios(&sizing.pane_ratios, sizing.session_count);
                return terminal_size_for_split_session(
                    sizing.viewport,
                    sizing.font_size,
                    sizing.padding,
                    sizing.sidebar_width,
                    ratios[session_index.min(sizing.session_count - 1)],
                    SplitOrientation::Columns,
                    sizing.session_count,
                );
            }
            TileLayout::Rows => {
                let ratios = normalize_pane_ratios(&sizing.pane_ratios, sizing.session_count);
                return terminal_size_for_split_session(
                    sizing.viewport,
                    sizing.font_size,
                    sizing.padding,
                    sizing.sidebar_width,
                    ratios[session_index.min(sizing.session_count - 1)],
                    SplitOrientation::Rows,
                    sizing.session_count,
                );
            }
            TileLayout::Grid => {}
        }
    }

    terminal_size_for_viewport(
        sizing.viewport,
        sizing.font_size,
        sizing.padding,
        sizing.sidebar_width,
        sizing.tile_layout,
        sizing.session_count,
        sizing.tiled,
    )
}

fn terminal_content_bounds_for_session(
    sizing: TerminalSizing,
    session_index: usize,
) -> TerminalContentBounds {
    let size = terminal_size_for_session(sizing.clone(), session_index);
    let cell_width = terminal_char_width(sizing.font_size);
    let line_height = terminal_line_height(sizing.font_size);
    let content_width = (sizing.viewport.width.as_f32() - sizing.sidebar_width).max(1.0);
    let content_height = (sizing.viewport.height.as_f32() - TITLEBAR_HEIGHT).max(1.0);
    let padding = sizing.padding;

    let (pane_x, pane_y) = if sizing.tiled && sizing.session_count >= 2 {
        match sizing.tile_layout {
            TileLayout::Columns => {
                let ratios = normalize_pane_ratios(&sizing.pane_ratios, sizing.session_count);
                let pane_index = session_index.min(sizing.session_count - 1);
                let split_width = (content_width
                    - RESIZE_HANDLE_SIZE * (sizing.session_count - 1) as f32)
                    .max(1.0);
                let offset = ratios.iter().take(pane_index).sum::<f32>() * split_width
                    + RESIZE_HANDLE_SIZE * pane_index as f32;
                (sizing.sidebar_width + offset, TITLEBAR_HEIGHT)
            }
            TileLayout::Rows => {
                let ratios = normalize_pane_ratios(&sizing.pane_ratios, sizing.session_count);
                let pane_index = session_index.min(sizing.session_count - 1);
                let split_height = (content_height
                    - RESIZE_HANDLE_SIZE * (sizing.session_count - 1) as f32)
                    .max(1.0);
                let offset = ratios.iter().take(pane_index).sum::<f32>() * split_height
                    + RESIZE_HANDLE_SIZE * pane_index as f32;
                (sizing.sidebar_width, TITLEBAR_HEIGHT + offset)
            }
            TileLayout::Grid => {
                grid_pane_origin(sizing, session_index, content_width, content_height)
            }
        }
    } else if sizing.tiled {
        grid_pane_origin(sizing, session_index, content_width, content_height)
    } else {
        (sizing.sidebar_width, TITLEBAR_HEIGHT)
    };

    TerminalContentBounds {
        origin: Point {
            x: px(pane_x + padding),
            y: px(pane_y + padding),
        },
        columns: size.columns,
        rows: size.rows,
        cell_width,
        line_height,
    }
}

fn grid_pane_origin(
    sizing: TerminalSizing,
    session_index: usize,
    content_width: f32,
    content_height: f32,
) -> (f32, f32) {
    let pane_count = if sizing.tiled {
        sizing.session_count.max(1)
    } else {
        1
    };
    let columns = tile_columns_for_layout(pane_count, sizing.tile_layout).max(1);
    let rows = pane_count.div_ceil(columns).max(1);
    let column = session_index.min(pane_count - 1) % columns;
    let row = session_index.min(pane_count - 1) / columns;
    let tile_width = content_width / columns as f32;
    let tile_height = content_height / rows as f32;

    (
        sizing.sidebar_width + tile_width * column as f32,
        TITLEBAR_HEIGHT + tile_height * row as f32,
    )
}

fn terminal_size_for_split_session(
    viewport: Size<Pixels>,
    terminal_font_size: f32,
    terminal_padding: f32,
    sidebar_width: f32,
    ratio: f32,
    orientation: SplitOrientation,
    pane_count: usize,
) -> TerminalSize {
    let ratio = ratio.clamp(MIN_PANE_RATIO, 1.0);
    let handle_space = RESIZE_HANDLE_SIZE * pane_count.saturating_sub(1) as f32;
    let width = viewport.width.as_f32();
    let height = viewport.height.as_f32();
    let (pane_width, pane_height) = match orientation {
        SplitOrientation::Columns => (
            ((width - sidebar_width - handle_space).max(1.0) * ratio).max(160.0),
            (height - TITLEBAR_HEIGHT).max(1.0),
        ),
        SplitOrientation::Rows => (
            (width - sidebar_width).max(160.0),
            ((height - TITLEBAR_HEIGHT - handle_space).max(1.0) * ratio).max(96.0),
        ),
    };
    let terminal_width = (pane_width - (terminal_padding * 2.0)).max(160.0);
    let terminal_height = (pane_height - STATUSLINE_HEIGHT - (terminal_padding * 2.0)).max(96.0);
    let columns = (terminal_width / terminal_char_width(terminal_font_size))
        .floor()
        .max(20.0) as u16;
    let rows = (terminal_height / terminal_line_height(terminal_font_size))
        .floor()
        .max(5.0) as u16;

    TerminalSize::new(columns, rows)
}

fn app_state_dir() -> PathBuf {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local_app_data).join("lazyterm");
    }

    if let Some(xdg_state_home) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(xdg_state_home).join("lazyterm");
    }

    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("lazyterm");
    }

    PathBuf::from(".lazyterm")
}

fn default_terminal_padding() -> f32 {
    DEFAULT_TERMINAL_PADDING
}

fn default_tile_layout() -> TileLayout {
    TileLayout::Grid
}

fn default_rail_width() -> RailWidth {
    RailWidth::Compact
}

fn sidebar_width_for_rail(width: RailWidth) -> f32 {
    match width {
        RailWidth::Compact => COMPACT_SIDEBAR_WIDTH,
        RailWidth::Default => DEFAULT_SIDEBAR_WIDTH,
        RailWidth::Wide => WIDE_SIDEBAR_WIDTH,
    }
}

fn default_split_ratio() -> f32 {
    DEFAULT_SPLIT_RATIO
}

fn load_ui_settings(state_dir: &Path) -> Option<UiSettings> {
    let path = state_dir.join("ui-settings.json");
    let payload = fs::read_to_string(path).ok()?;
    serde_json::from_str::<PersistedUiSettings>(&payload)
        .ok()
        .map(Into::into)
}

fn save_ui_settings(
    state_dir: &Path,
    settings: UiSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(state_dir)?;
    let payload = serde_json::to_string_pretty(&PersistedUiSettings::from(settings))?;
    fs::write(state_dir.join("ui-settings.json"), payload)?;
    Ok(())
}

fn load_session_summaries(state_dir: &Path) -> Option<Vec<SessionSummary>> {
    fs::create_dir_all(state_dir).ok()?;
    SessionStore::open(state_dir.join("sessions.sqlite"))
        .ok()?
        .list()
        .ok()
}

fn save_session_summaries(
    state_dir: &Path,
    summaries: &[SessionSummary],
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(state_dir)?;
    let mut store = SessionStore::open(state_dir.join("sessions.sqlite"))?;
    store.replace_all(summaries)?;
    Ok(())
}

fn spawn_persisted_sessions(
    summaries: Vec<SessionSummary>,
    fallback_branch: Option<String>,
) -> Vec<TerminalSession> {
    summaries
        .into_iter()
        .enumerate()
        .map(|(index, summary)| {
            TerminalSession::spawn(
                index + 1,
                summary.workspace.cwd,
                summary
                    .workspace
                    .git_branch
                    .or_else(|| fallback_branch.clone()),
                summary.title,
                summary.agent,
                None,
            )
        })
        .collect::<Vec<_>>()
}

fn start_api_listener(sender: Sender<ApiEvent>) {
    thread::Builder::new()
        .name("lazyterm-api-listener".into())
        .spawn(move || {
            let listener = match TcpListener::bind(API_BIND_ADDR) {
                Ok(listener) => listener,
                Err(error) => {
                    eprintln!("lazyterm: failed to bind api listener on {API_BIND_ADDR}: {error}");
                    return;
                }
            };

            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => handle_api_stream(stream, &sender),
                    Err(error) => eprintln!("lazyterm: api connection failed: {error}"),
                }
            }
        })
        .expect("spawn api listener thread");
}

fn handle_api_stream(mut stream: TcpStream, sender: &Sender<ApiEvent>) {
    let request = match read_api_request(&stream) {
        Ok(request) => request,
        Err(error) => {
            let _ = write_api_response(
                &mut stream,
                &ApiResponse::Error {
                    message: error.to_string(),
                },
            );
            return;
        }
    };

    let (response_sender, response_receiver) = mpsc::channel();
    if sender
        .send(ApiEvent {
            request,
            response: response_sender,
        })
        .is_err()
    {
        let _ = write_api_response(
            &mut stream,
            &ApiResponse::Error {
                message: "app event loop is closed".into(),
            },
        );
        return;
    }

    match response_receiver.recv_timeout(API_RESPONSE_TIMEOUT) {
        Ok(response) => {
            let _ = write_api_response(&mut stream, &response);
        }
        Err(error) => {
            let _ = write_api_response(
                &mut stream,
                &ApiResponse::Error {
                    message: format!("timed out waiting for app response: {error}"),
                },
            );
        }
    }
}

fn read_api_request(stream: &TcpStream) -> io::Result<ApiRequest> {
    let mut line = String::new();
    let mut reader = BufReader::new(stream.try_clone()?);
    reader.read_line(&mut line)?;
    serde_json::from_str(line.trim_end()).map_err(io::Error::other)
}

fn write_api_response(stream: &mut TcpStream, response: &ApiResponse) -> io::Result<()> {
    serde_json::to_writer(&mut *stream, response).map_err(io::Error::other)?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn tile_columns_for_layout(session_count: usize, layout: TileLayout) -> usize {
    match layout {
        TileLayout::Grid => match session_count {
            0 | 1 => 1,
            2..=4 => 2,
            _ => 3,
        },
        TileLayout::Columns => session_count.max(1),
        TileLayout::Rows => 1,
    }
}

fn pane_ratios_for_count(settings: &UiSettings, pane_count: usize) -> Vec<f32> {
    if pane_count == 2 && settings.pane_ratios.len() != 2 {
        return normalize_pane_ratios(&[settings.split_ratio, 1.0 - settings.split_ratio], 2);
    }

    normalize_pane_ratios(&settings.pane_ratios, pane_count)
}

fn normalize_pane_ratios(ratios: &[f32], pane_count: usize) -> Vec<f32> {
    if pane_count == 0 {
        return Vec::new();
    }

    if ratios.len() != pane_count {
        return vec![1.0 / pane_count as f32; pane_count];
    }

    let mut normalized = ratios
        .iter()
        .map(|ratio| ratio.max(MIN_PANE_RATIO))
        .collect::<Vec<_>>();
    let total = normalized.iter().sum::<f32>();
    if total <= f32::EPSILON {
        return vec![1.0 / pane_count as f32; pane_count];
    }

    for ratio in &mut normalized {
        *ratio /= total;
    }

    normalized
}

fn directional_session_index(
    active: usize,
    session_count: usize,
    columns: usize,
    direction: PaneDirection,
) -> Option<usize> {
    if session_count <= 1 || columns == 0 || active >= session_count {
        return None;
    }

    let rows = session_count.div_ceil(columns);
    let row = active / columns;
    let column = active % columns;
    let next = match direction {
        PaneDirection::Left if column > 0 => Some(active - 1),
        PaneDirection::Right if column + 1 < columns => Some(active + 1),
        PaneDirection::Up if row > 0 => Some(active - columns),
        PaneDirection::Down if row + 1 < rows => Some((active + columns).min(session_count - 1)),
        _ => None,
    }?;

    (next < session_count).then_some(next)
}

fn terminal_char_width(font_size: f32) -> f32 {
    TERMINAL_CHAR_WIDTH * (font_size / 12.0)
}

fn terminal_line_height(font_size: f32) -> f32 {
    TERMINAL_LINE_HEIGHT * (font_size / 12.0)
}

fn terminal_key_bytes(key: &str, modifiers: gpui::Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let modified = modified_key_code(modifiers);

    match key {
        "enter" => Some(b"\r".to_vec()),
        "backspace" if modifiers.control => Some(b"\x17".to_vec()),
        "backspace" if modifiers.alt => Some(b"\x1b\x7f".to_vec()),
        "backspace" => Some(b"\x7f".to_vec()),
        "escape" => Some(b"\x1b".to_vec()),
        "tab" if modifiers.shift => Some(b"\x1b[Z".to_vec()),
        "tab" => Some(b"\t".to_vec()),
        "delete" => {
            Some(modified_tilde_key_bytes(3, modified).unwrap_or_else(|| b"\x1b[3~".to_vec()))
        }
        "left" => Some(cursor_key_bytes(b'D', app_cursor, modified)),
        "right" => Some(cursor_key_bytes(b'C', app_cursor, modified)),
        "up" => Some(cursor_key_bytes(b'A', app_cursor, modified)),
        "down" => Some(cursor_key_bytes(b'B', app_cursor, modified)),
        "home" => Some(cursor_key_bytes(b'H', app_cursor, modified)),
        "end" => Some(cursor_key_bytes(b'F', app_cursor, modified)),
        "pageup" => {
            Some(modified_tilde_key_bytes(5, modified).unwrap_or_else(|| b"\x1b[5~".to_vec()))
        }
        "pagedown" => {
            Some(modified_tilde_key_bytes(6, modified).unwrap_or_else(|| b"\x1b[6~".to_vec()))
        }
        "insert" => {
            Some(modified_tilde_key_bytes(2, modified).unwrap_or_else(|| b"\x1b[2~".to_vec()))
        }
        "f1" => Some(b"\x1bOP".to_vec()),
        "f2" => Some(b"\x1bOQ".to_vec()),
        "f3" => Some(b"\x1bOR".to_vec()),
        "f4" => Some(b"\x1bOS".to_vec()),
        "f5" => Some(b"\x1b[15~".to_vec()),
        "f6" => Some(b"\x1b[17~".to_vec()),
        "f7" => Some(b"\x1b[18~".to_vec()),
        "f8" => Some(b"\x1b[19~".to_vec()),
        "f9" => Some(b"\x1b[20~".to_vec()),
        "f10" => Some(b"\x1b[21~".to_vec()),
        "f11" => Some(b"\x1b[23~".to_vec()),
        "f12" => Some(b"\x1b[24~".to_vec()),
        _ => None,
    }
}

fn modified_key_code(modifiers: gpui::Modifiers) -> Option<u8> {
    match (modifiers.shift, modifiers.alt, modifiers.control) {
        (false, false, false) => None,
        (true, false, false) => Some(2),
        (false, true, false) => Some(3),
        (true, true, false) => Some(4),
        (false, false, true) => Some(5),
        (true, false, true) => Some(6),
        (false, true, true) => Some(7),
        (true, true, true) => Some(8),
    }
}

fn modified_tilde_key_bytes(prefix: u8, modified: Option<u8>) -> Option<Vec<u8>> {
    modified.map(|code| format!("\x1b[{prefix};{code}~").into_bytes())
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

fn app_shortcut_modifiers(modifiers: gpui::Modifiers) -> bool {
    (modifiers.control && modifiers.shift) || (cfg!(target_os = "macos") && modifiers.platform)
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

fn status_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Running => "running",
        SessionStatus::Waiting => "waiting",
        SessionStatus::NeedsInput => "needs input",
        SessionStatus::Failed => "failed",
        SessionStatus::Done => "done",
    }
}

fn command_detail(command: &CommandItem) -> String {
    if !command.shortcut.is_empty() {
        return command.shortcut.clone();
    }

    match command.meta.as_str() {
        "ready" => "installed".into(),
        "missing" => "missing".into(),
        _ => command.meta.clone(),
    }
}

fn session_needs_attention(session: &TerminalSession) -> bool {
    matches!(
        session.summary.status,
        SessionStatus::NeedsInput | SessionStatus::Failed
    )
}

fn notification_for_status(status: SessionStatus, output: &str) -> Option<String> {
    match status {
        SessionStatus::Running | SessionStatus::Waiting | SessionStatus::NeedsInput => None,
        SessionStatus::Failed => Some(first_non_empty_line(output).unwrap_or("failed").into()),
        SessionStatus::Done => None,
    }
}

fn alternate_scroll_bytes(lines: i32) -> Vec<u8> {
    let command = if lines > 0 { b'A' } else { b'B' };
    let mut bytes = Vec::with_capacity(lines.unsigned_abs() as usize * 3);
    for _ in 0..lines.abs() {
        bytes.extend_from_slice(&[0x1b, b'O', command]);
    }
    bytes
}

fn terminal_point_for_mouse(
    position: Point<Pixels>,
    bounds: TerminalContentBounds,
) -> Option<(u16, u16)> {
    let x = position.x.as_f32() - bounds.origin.x.as_f32();
    let y = position.y.as_f32() - bounds.origin.y.as_f32();
    if x < 0.0 || y < 0.0 || bounds.columns == 0 || bounds.rows == 0 {
        return None;
    }

    let column = ((x / bounds.cell_width).floor() as u16).min(bounds.columns - 1);
    let row = ((y / bounds.line_height).floor() as u16).min(bounds.rows - 1);
    Some((column, row))
}

fn terminal_mouse_mode_enabled(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

fn terminal_mouse_move_enabled(mode: TermMode, dragging: bool) -> bool {
    if mode.contains(TermMode::MOUSE_MOTION) {
        return true;
    }

    dragging && mode.contains(TermMode::MOUSE_DRAG)
}

fn terminal_mouse_report_bytes(
    column: u16,
    row: u16,
    button: Option<MouseButton>,
    action: MouseReportAction,
    modifiers: gpui::Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let code = mouse_button_code(button, action)?;
    let code = code + mouse_modifier_code(modifiers);
    if mode.contains(TermMode::SGR_MOUSE) {
        return Some(sgr_mouse_report(
            column,
            row,
            code,
            action != MouseReportAction::Release,
        ));
    }

    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let normal_code = if action == MouseReportAction::Release {
        3 + mouse_modifier_code(modifiers)
    } else {
        code
    };
    normal_mouse_report(column, row, normal_code, utf8)
}

fn terminal_mouse_scroll_report_bytes(
    column: u16,
    row: u16,
    lines: i32,
    modifiers: gpui::Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if lines == 0 {
        return None;
    }

    let action = if lines > 0 {
        MouseReportAction::ScrollUp
    } else {
        MouseReportAction::ScrollDown
    };
    let report = terminal_mouse_report_bytes(column, row, None, action, modifiers, mode)?;
    let mut bytes = Vec::with_capacity(report.len() * lines.unsigned_abs() as usize);
    for _ in 0..lines.abs() {
        bytes.extend_from_slice(&report);
    }
    Some(bytes)
}

fn mouse_button_code(button: Option<MouseButton>, action: MouseReportAction) -> Option<u8> {
    match action {
        MouseReportAction::Press => match button? {
            MouseButton::Left => Some(0),
            MouseButton::Middle => Some(1),
            MouseButton::Right => Some(2),
            MouseButton::Navigate(_) => None,
        },
        MouseReportAction::Release => match button {
            Some(MouseButton::Left) => Some(0),
            Some(MouseButton::Middle) => Some(1),
            Some(MouseButton::Right) => Some(2),
            Some(MouseButton::Navigate(_)) => None,
            None => Some(3),
        },
        MouseReportAction::Move => match button {
            Some(MouseButton::Left) => Some(32),
            Some(MouseButton::Middle) => Some(33),
            Some(MouseButton::Right) => Some(34),
            Some(MouseButton::Navigate(_)) => None,
            None => Some(35),
        },
        MouseReportAction::ScrollUp => Some(64),
        MouseReportAction::ScrollDown => Some(65),
    }
}

fn mouse_modifier_code(modifiers: gpui::Modifiers) -> u8 {
    let mut code = 0;
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if modifiers.control {
        code += 16;
    }
    code
}

fn normal_mouse_report(column: u16, row: u16, code: u8, utf8: bool) -> Option<Vec<u8>> {
    let max_point = if utf8 { 2015 } else { 223 };
    if column >= max_point || row >= max_point {
        return None;
    }

    let mut bytes = vec![b'\x1b', b'[', b'M', 32 + code];
    append_normal_mouse_position(&mut bytes, column as usize, utf8);
    append_normal_mouse_position(&mut bytes, row as usize, utf8);
    Some(bytes)
}

fn append_normal_mouse_position(bytes: &mut Vec<u8>, position: usize, utf8: bool) {
    if utf8 && position >= 95 {
        let encoded = 32 + 1 + position;
        bytes.push((0xc0 + encoded / 64) as u8);
        bytes.push((0x80 + (encoded & 63)) as u8);
    } else {
        bytes.push(32 + 1 + position as u8);
    }
}

fn sgr_mouse_report(column: u16, row: u16, code: u8, pressed: bool) -> Vec<u8> {
    let terminator = if pressed { 'M' } else { 'm' };
    format!("\x1b[<{code};{};{}{terminator}", column + 1, row + 1).into_bytes()
}

fn paste_bytes_for_terminal(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let sanitized = text.replace('\x1b', "");
        return format!("\x1b[200~{sanitized}\x1b[201~").into_bytes();
    }

    text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
}

fn cursor_key_bytes(key: u8, app_cursor: bool, modified: Option<u8>) -> Vec<u8> {
    if let Some(code) = modified {
        return format!("\x1b[1;{code}{}", key as char).into_bytes();
    }

    match (key, app_cursor) {
        (b'A', true) => b"\x1bOA".to_vec(),
        (b'A', false) => b"\x1b[A".to_vec(),
        (b'B', true) => b"\x1bOB".to_vec(),
        (b'B', false) => b"\x1b[B".to_vec(),
        (b'C', true) => b"\x1bOC".to_vec(),
        (b'C', false) => b"\x1b[C".to_vec(),
        (b'D', true) => b"\x1bOD".to_vec(),
        (b'D', false) => b"\x1b[D".to_vec(),
        (b'H', true) => b"\x1bOH".to_vec(),
        (b'H', false) => b"\x1b[H".to_vec(),
        (b'F', true) => b"\x1bOF".to_vec(),
        (b'F', false) => b"\x1b[F".to_vec(),
        _ => Vec::new(),
    }
}

fn first_non_empty_line(output: &str) -> Option<&str> {
    output.lines().map(str::trim).find(|line| !line.is_empty())
}

fn agent_preset(agent: AgentKind) -> Option<&'static AgentPreset> {
    AGENT_PRESETS.iter().find(|preset| preset.kind == agent)
}

fn command_for_agent(agent: AgentKind) -> ShellCommand {
    if agent == AgentKind::Shell {
        return ShellCommand::default_for_platform();
    }

    let Some(preset) = agent_preset(agent) else {
        return ShellCommand::default_for_platform();
    };
    ShellCommand {
        program: preset.command.into(),
        args: preset.args.iter().map(|arg| (*arg).into()).collect(),
        cwd: None,
    }
}

fn command_label(command: &ShellCommand) -> String {
    if command.args.is_empty() {
        return command.program.clone();
    }

    format!("{} {}", command.program, command.args.join(" "))
}

fn agent_health_summaries() -> Vec<AgentHealthSummary> {
    AGENT_PRESETS
        .iter()
        .map(|preset| {
            let command = if preset.kind == AgentKind::Shell {
                command_for_agent(AgentKind::Shell).program
            } else {
                preset.command.to_string()
            };
            let available = preset.kind == AgentKind::Shell || executable_exists(&command);

            AgentHealthSummary {
                agent: preset.kind,
                command,
                available,
            }
        })
        .collect()
}

fn agent_health_label(agent: AgentKind) -> &'static str {
    if agent == AgentKind::Shell {
        return "ready";
    }

    let Some(preset) = agent_preset(agent) else {
        return "missing";
    };

    if executable_exists(preset.command) {
        "ready"
    } else {
        "missing"
    }
}

fn executable_exists(program: &str) -> bool {
    executable_candidates(program)
        .into_iter()
        .any(|candidate| executable_file_exists(&candidate))
}

#[cfg(windows)]
fn executable_file_exists(path: &Path) -> bool {
    path.is_file()
}

#[cfg(not(windows))]
fn executable_file_exists(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn executable_candidates(program: &str) -> Vec<PathBuf> {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return executable_suffixes(program)
            .into_iter()
            .map(|suffix| PathBuf::from(format!("{program}{suffix}")))
            .collect();
    }

    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };

    std::env::split_paths(&path)
        .flat_map(|dir| {
            executable_suffixes(program)
                .into_iter()
                .map(move |suffix| dir.join(format!("{program}{suffix}")))
        })
        .collect()
}

fn executable_suffixes(program: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let mut suffixes = vec![String::new()];
        if Path::new(program).extension().is_none() {
            let pathext =
                std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD;.PS1".into());
            suffixes.extend(
                pathext
                    .split(';')
                    .filter(|suffix| !suffix.is_empty())
                    .map(|suffix| suffix.to_ascii_lowercase()),
            );
        }
        suffixes
    }

    #[cfg(not(windows))]
    {
        let _ = program;
        vec![String::new()]
    }
}

fn parse_agent_launch_query(query: &str, default_cwd: &Path) -> Option<AgentLaunchQuery> {
    let mut parts = query.split_whitespace();
    let agent = agent_kind_from_palette_token(parts.next()?)?;
    let mut cwd = default_cwd.to_path_buf();
    let mut task = Vec::new();

    while let Some(part) = parts.next() {
        match part {
            "--cwd" | "-C" => {
                cwd = resolve_palette_cwd(default_cwd, parts.next()?);
            }
            "--task" | "-t" | "--" => {
                task.extend(parts.map(str::to_string));
                break;
            }
            _ if part.starts_with('@') && part.len() > 1 => {
                cwd = resolve_palette_cwd(default_cwd, &part[1..]);
            }
            _ => task.push(part.to_string()),
        }
    }

    Some(AgentLaunchQuery {
        agent,
        cwd,
        task: (!task.is_empty()).then(|| task.join(" ")),
    })
}

fn agent_kind_from_palette_token(token: &str) -> Option<AgentKind> {
    match token.trim_end_matches(':').to_ascii_lowercase().as_str() {
        "shell" | "sh" => Some(AgentKind::Shell),
        "codex" | "cx" => Some(AgentKind::Codex),
        "claude" | "cc" => Some(AgentKind::Claude),
        "opencode" | "open-code" | "open_code" | "oc" => Some(AgentKind::OpenCode),
        "gemini" | "gm" => Some(AgentKind::Gemini),
        "aider" | "ad" => Some(AgentKind::Aider),
        _ => None,
    }
}

fn resolve_palette_cwd(default_cwd: &Path, value: &str) -> PathBuf {
    let value = value.trim_matches('"');
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        default_cwd.join(path)
    }
}

fn startup_input_bytes(task: String) -> Vec<u8> {
    let mut input = task.into_bytes();
    input.push(b'\r');
    input
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
    current_branch_for(Path::new("."))
}

fn current_branch_for(cwd: &Path) -> Option<String> {
    std::process::Command::new("git")
        .current_dir(cwd)
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
    use std::time::{SystemTime, UNIX_EPOCH};

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
    fn app_shortcuts_do_not_steal_plain_control_keys() {
        assert!(!app_shortcut_modifiers(gpui::Modifiers {
            control: true,
            ..Default::default()
        }));
        assert!(app_shortcut_modifiers(gpui::Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        }));
    }

    #[test]
    fn agent_commands_map_to_cli_programs() {
        for preset in AGENT_PRESETS
            .iter()
            .filter(|preset| preset.kind != AgentKind::Shell)
        {
            let command = command_for_agent(preset.kind);
            assert_eq!(command.program, preset.command);
            assert_eq!(
                command.args,
                preset
                    .args
                    .iter()
                    .map(|arg| (*arg).to_string())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn agent_health_reports_all_presets() {
        let health = agent_health_summaries();

        assert_eq!(health.len(), AGENT_PRESETS.len());
        assert!(health.iter().any(|item| item.agent == AgentKind::Shell));
        assert!(health.iter().any(|item| item.command == "codex"));
    }

    #[test]
    fn executable_candidates_expand_windows_pathext() {
        let candidates = executable_candidates("codex");

        assert!(candidates
            .iter()
            .any(|candidate| candidate.ends_with("codex")));
        if cfg!(windows) {
            assert!(candidates
                .iter()
                .any(|candidate| candidate.ends_with("codex.exe")));
            assert!(candidates
                .iter()
                .any(|candidate| candidate.ends_with("codex.cmd")));
        }
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
    fn icon_assets_are_stable_svg_paths() {
        assert_eq!(IconKind::NewPane.asset_path(), "icons/plus.svg");
        assert_eq!(IconKind::SplitLayout.asset_path(), "icons/split.svg");
        assert_eq!(IconKind::CommandPalette.asset_path(), "icons/command.svg");
        assert_eq!(IconKind::Close.asset_path(), "icons/close.svg");
    }

    #[test]
    fn palette_agent_launch_query_parses_task_text() {
        let launch = parse_agent_launch_query("codex fix the parser", Path::new("C:/repo"))
            .expect("agent launch query should parse");

        assert_eq!(
            launch,
            AgentLaunchQuery {
                agent: AgentKind::Codex,
                cwd: PathBuf::from("C:/repo"),
                task: Some("fix the parser".into()),
            }
        );
    }

    #[test]
    fn palette_agent_launch_query_parses_cwd_options() {
        let launch =
            parse_agent_launch_query("claude --cwd crates/ui review this", Path::new("C:/repo"))
                .expect("agent launch query with cwd should parse");

        assert_eq!(launch.agent, AgentKind::Claude);
        assert_eq!(launch.cwd, PathBuf::from("C:/repo").join("crates/ui"));
        assert_eq!(launch.task, Some("review this".into()));
    }

    #[test]
    fn palette_agent_launch_query_parses_at_cwd_shorthand() {
        let launch =
            parse_agent_launch_query("opencode @crates/ui implement resize", Path::new("C:/repo"))
                .expect("agent launch query with shorthand cwd should parse");

        assert_eq!(launch.agent, AgentKind::OpenCode);
        assert_eq!(launch.cwd, PathBuf::from("C:/repo").join("crates/ui"));
        assert_eq!(launch.task, Some("implement resize".into()));
    }

    #[test]
    fn status_notifications_are_terminal_sized() {
        assert_eq!(
            notification_for_status(SessionStatus::NeedsInput, "approve?"),
            None
        );
        assert_eq!(
            notification_for_status(SessionStatus::Failed, "error: bad\nmore"),
            Some("error: bad".into())
        );
        assert_eq!(notification_for_status(SessionStatus::Done, "done"), None);
        assert_eq!(notification_for_status(SessionStatus::Running, "ok"), None);
    }

    #[test]
    fn attention_only_tracks_actionable_statuses() {
        let (_sender, events) = mpsc::channel();
        let mut session = TerminalSession {
            summary: SessionSummary {
                id: SessionId::new("shell-99"),
                title: "test".into(),
                agent: AgentKind::Shell,
                status: SessionStatus::Running,
                workspace: WorkspaceRef {
                    cwd: PathBuf::from("."),
                    git_branch: Some("main".into()),
                },
                command: "test".into(),
                last_activity: "test".into(),
                notification: None,
            },
            pty: None,
            events,
            terminal: TerminalGrid::new(TerminalSize::DEFAULT),
            lines: Vec::new(),
            pending_line: String::new(),
            pending_startup_input: None,
            terminal_size: TerminalSize::DEFAULT,
        };

        session.summary.status = SessionStatus::Done;
        session.summary.notification = Some("done".into());
        assert!(!session_needs_attention(&session));

        session.summary.status = SessionStatus::NeedsInput;
        assert!(session_needs_attention(&session));
    }

    #[test]
    fn api_response_serializes_as_json_line() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener binds");
        let address = listener.local_addr().expect("listener has address");
        let client = TcpStream::connect(address).expect("client connects");
        let (mut server, _) = listener.accept().expect("server accepts");

        write_api_response(&mut server, &ApiResponse::Ack).expect("response writes");

        let mut line = String::new();
        BufReader::new(client)
            .read_line(&mut line)
            .expect("client reads response");

        assert_eq!(line.trim_end(), "\"Ack\"");
    }

    #[test]
    fn api_request_reads_json_line() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener binds");
        let address = listener.local_addr().expect("listener has address");
        let mut client = TcpStream::connect(address).expect("client connects");
        let (server, _) = listener.accept().expect("server accepts");

        writeln!(
            client,
            "{}",
            serde_json::to_string(&ApiRequest::ListSessions).expect("request serializes")
        )
        .expect("client writes request");

        assert_eq!(
            read_api_request(&server).expect("request reads"),
            ApiRequest::ListSessions
        );
    }

    #[test]
    fn ui_settings_persist_to_disk() {
        let state_dir = test_state_dir("ui-settings");
        let settings = UiSettings {
            tile_sessions: true,
            tile_layout: TileLayout::Columns,
            rail_width: RailWidth::Wide,
            terminal_font_size: 15.0,
            terminal_padding: 22.0,
            split_ratio: 0.65,
            pane_ratios: vec![0.65, 0.35],
        };

        save_ui_settings(&state_dir, settings.clone()).expect("settings save");

        let loaded = load_ui_settings(&state_dir).expect("settings load");
        assert_eq!(loaded.tile_sessions, settings.tile_sessions);
        assert_eq!(loaded.tile_layout, settings.tile_layout);
        assert_eq!(loaded.rail_width, settings.rail_width);
        assert_eq!(loaded.terminal_font_size, settings.terminal_font_size);
        assert_eq!(loaded.terminal_padding, settings.terminal_padding);
        assert_eq!(loaded.split_ratio, settings.split_ratio);
        assert_eq!(loaded.pane_ratios, settings.pane_ratios);
        let _ = fs::remove_dir_all(state_dir);
    }

    #[test]
    fn legacy_ui_settings_default_to_compact_rail() {
        let state_dir = test_state_dir("legacy-ui-settings");
        fs::create_dir_all(&state_dir).expect("state dir");
        fs::write(
            state_dir.join("ui-settings.json"),
            r#"{
  "tile_sessions": false,
  "tile_layout": "Grid",
  "terminal_font_size": 12.0,
  "terminal_padding": 14.0,
  "split_ratio": 0.5,
  "pane_ratios": []
}"#,
        )
        .expect("write legacy settings");

        let loaded = load_ui_settings(&state_dir).expect("settings load");

        assert_eq!(loaded.rail_width, RailWidth::Compact);
        let _ = fs::remove_dir_all(state_dir);
    }

    #[test]
    fn session_summaries_persist_to_disk() {
        let state_dir = test_state_dir("sessions");
        let summary = SessionSummary {
            id: SessionId::new("shell-1"),
            title: "shell 1".into(),
            agent: AgentKind::Shell,
            status: SessionStatus::Running,
            workspace: WorkspaceRef {
                cwd: PathBuf::from("."),
                git_branch: Some("main".into()),
            },
            command: "pwsh.exe -NoLogo -NoProfile".into(),
            last_activity: "attached".into(),
            notification: None,
        };

        save_session_summaries(&state_dir, std::slice::from_ref(&summary)).expect("sessions save");

        assert_eq!(
            load_session_summaries(&state_dir).expect("sessions load"),
            vec![summary]
        );
        let _ = fs::remove_dir_all(state_dir);
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
    fn directional_navigation_tracks_grid_neighbors() {
        assert_eq!(
            directional_session_index(0, 4, 2, PaneDirection::Right),
            Some(1)
        );
        assert_eq!(
            directional_session_index(0, 4, 2, PaneDirection::Down),
            Some(2)
        );
        assert_eq!(
            directional_session_index(0, 4, 2, PaneDirection::Left),
            None
        );
        assert_eq!(
            directional_session_index(3, 5, 3, PaneDirection::Down),
            None
        );
        assert_eq!(
            directional_session_index(2, 5, 3, PaneDirection::Down),
            Some(4)
        );
    }

    #[test]
    fn tile_layouts_choose_expected_column_counts() {
        assert_eq!(tile_columns_for_layout(5, TileLayout::Grid), 3);
        assert_eq!(tile_columns_for_layout(5, TileLayout::Columns), 5);
        assert_eq!(tile_columns_for_layout(5, TileLayout::Rows), 1);
    }

    #[test]
    fn pane_ratios_normalize_for_dynamic_pane_counts() {
        assert_eq!(normalize_pane_ratios(&[], 3), vec![1.0 / 3.0; 3]);
        assert_eq!(
            normalize_pane_ratios(&[2.0, 1.0], 2),
            vec![2.0 / 3.0, 1.0 / 3.0]
        );
    }

    #[test]
    fn terminal_size_tracks_tile_layout() {
        let viewport = Size {
            width: px(1180.0),
            height: px(760.0),
        };
        let columns = terminal_size_for_viewport(
            viewport,
            12.0,
            DEFAULT_TERMINAL_PADDING,
            DEFAULT_SIDEBAR_WIDTH,
            TileLayout::Columns,
            3,
            true,
        );
        let rows = terminal_size_for_viewport(
            viewport,
            12.0,
            DEFAULT_TERMINAL_PADDING,
            DEFAULT_SIDEBAR_WIDTH,
            TileLayout::Rows,
            3,
            true,
        );

        assert!(columns.columns < rows.columns);
        assert!(rows.rows < columns.rows);
    }

    #[test]
    fn terminal_size_tracks_split_ratio_for_two_panes() {
        let viewport = Size {
            width: px(1180.0),
            height: px(760.0),
        };
        let sizing = TerminalSizing {
            viewport,
            font_size: 12.0,
            padding: DEFAULT_TERMINAL_PADDING,
            tile_layout: TileLayout::Columns,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            pane_ratios: vec![0.7, 0.3],
            session_count: 2,
            tiled: true,
        };
        let first = terminal_size_for_session(sizing.clone(), 0);
        let second = terminal_size_for_session(sizing, 1);

        assert!(first.columns > second.columns);
        assert_eq!(first.rows, second.rows);
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
    fn terminal_grid_uses_native_scrollback() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));

        grid.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        assert_eq!(grid.display_offset(), 0);

        grid.scroll_display(Scroll::PageUp);
        assert!(grid.display_offset() > 0);

        grid.scroll_display(Scroll::Bottom);
        assert_eq!(grid.display_offset(), 0);
    }

    #[test]
    fn alternate_screen_scroll_maps_to_application_arrows() {
        assert_eq!(alternate_scroll_bytes(2), b"\x1bOA\x1bOA");
        assert_eq!(alternate_scroll_bytes(-1), b"\x1bOB");
        assert!(alternate_scroll_bytes(0).is_empty());
    }

    #[test]
    fn paste_normalizes_newlines_without_bracketed_paste() {
        assert_eq!(
            paste_bytes_for_terminal("one\r\ntwo\nthree", false),
            b"one\rtwo\rthree"
        );
    }

    #[test]
    fn paste_wraps_and_sanitizes_bracketed_paste() {
        assert_eq!(
            paste_bytes_for_terminal("one\x1b[31m\ntwo", true),
            b"\x1b[200~one[31m\ntwo\x1b[201~"
        );
    }

    #[test]
    fn terminal_grid_detects_alternate_scroll_mode() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));
        assert!(!grid.uses_alternate_scroll());

        grid.feed(b"\x1b[?1049h");
        assert!(grid.uses_alternate_scroll());
    }

    #[test]
    fn terminal_grid_detects_bracketed_paste_mode() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));
        assert!(!grid.uses_bracketed_paste());

        grid.feed(b"\x1b[?2004h");
        assert!(grid.uses_bracketed_paste());

        grid.feed(b"\x1b[?2004l");
        assert!(!grid.uses_bracketed_paste());
    }

    #[test]
    fn cursor_keys_follow_application_cursor_mode() {
        assert_eq!(cursor_key_bytes(b'A', false, None), b"\x1b[A");
        assert_eq!(cursor_key_bytes(b'A', true, None), b"\x1bOA");
        assert_eq!(cursor_key_bytes(b'D', false, None), b"\x1b[D");
        assert_eq!(cursor_key_bytes(b'D', true, None), b"\x1bOD");
        assert_eq!(cursor_key_bytes(b'H', false, None), b"\x1b[H");
        assert_eq!(cursor_key_bytes(b'H', true, None), b"\x1bOH");
        assert_eq!(cursor_key_bytes(b'F', false, None), b"\x1b[F");
        assert_eq!(cursor_key_bytes(b'F', true, None), b"\x1bOF");
    }

    #[test]
    fn modified_terminal_keys_use_xterm_sequences() {
        assert_eq!(
            terminal_key_bytes(
                "left",
                gpui::Modifiers {
                    control: true,
                    ..Default::default()
                },
                false
            ),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(
                "right",
                gpui::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                false
            ),
            Some(b"\x1b[1;3C".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(
                "delete",
                gpui::Modifiers {
                    control: true,
                    ..Default::default()
                },
                false
            ),
            Some(b"\x1b[3;5~".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(
                "backspace",
                gpui::Modifiers {
                    control: true,
                    ..Default::default()
                },
                false
            ),
            Some(b"\x17".to_vec())
        );
    }

    #[test]
    fn terminal_mouse_point_uses_terminal_content_origin() {
        let bounds = TerminalContentBounds {
            origin: Point {
                x: px(196.0),
                y: px(48.0),
            },
            columns: 80,
            rows: 24,
            cell_width: 8.0,
            line_height: 18.0,
        };

        assert_eq!(
            terminal_point_for_mouse(
                Point {
                    x: px(196.0),
                    y: px(48.0)
                },
                bounds
            ),
            Some((0, 0))
        );
        assert_eq!(
            terminal_point_for_mouse(
                Point {
                    x: px(220.0),
                    y: px(84.0)
                },
                bounds
            ),
            Some((3, 2))
        );
        assert_eq!(
            terminal_point_for_mouse(
                Point {
                    x: px(1200.0),
                    y: px(900.0)
                },
                bounds
            ),
            Some((79, 23))
        );
    }

    #[test]
    fn terminal_content_bounds_track_split_pane_origins() {
        let sizing = TerminalSizing {
            viewport: Size {
                width: px(1180.0),
                height: px(760.0),
            },
            font_size: 12.0,
            padding: DEFAULT_TERMINAL_PADDING,
            tile_layout: TileLayout::Columns,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            pane_ratios: vec![0.7, 0.3],
            session_count: 2,
            tiled: true,
        };

        let first = terminal_content_bounds_for_session(sizing.clone(), 0);
        let second = terminal_content_bounds_for_session(sizing, 1);

        assert_eq!(
            first.origin.x,
            px(DEFAULT_SIDEBAR_WIDTH + DEFAULT_TERMINAL_PADDING)
        );
        assert!(second.origin.x > first.origin.x);
        assert_eq!(first.origin.y, second.origin.y);
        assert!(first.columns > second.columns);
    }

    #[test]
    fn terminal_mouse_reports_sgr_press_and_release() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

        assert_eq!(
            terminal_mouse_report_bytes(
                2,
                4,
                Some(MouseButton::Left),
                MouseReportAction::Press,
                gpui::Modifiers::default(),
                mode,
            ),
            Some(b"\x1b[<0;3;5M".to_vec())
        );
        assert_eq!(
            terminal_mouse_report_bytes(
                2,
                4,
                Some(MouseButton::Left),
                MouseReportAction::Release,
                gpui::Modifiers::default(),
                mode,
            ),
            Some(b"\x1b[<0;3;5m".to_vec())
        );
    }

    #[test]
    fn terminal_mouse_reports_x10_press_and_drag() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG;

        assert_eq!(
            terminal_mouse_report_bytes(
                0,
                0,
                Some(MouseButton::Right),
                MouseReportAction::Press,
                gpui::Modifiers::default(),
                mode,
            ),
            Some(b"\x1b[M\"!!".to_vec())
        );
        assert_eq!(
            terminal_mouse_report_bytes(
                0,
                0,
                Some(MouseButton::Left),
                MouseReportAction::Move,
                gpui::Modifiers::default(),
                mode,
            ),
            Some(b"\x1b[M@!!".to_vec())
        );
        assert!(terminal_mouse_move_enabled(mode, true));
        assert!(!terminal_mouse_move_enabled(mode, false));
    }

    #[test]
    fn terminal_mouse_scroll_report_repeats_wheel_button() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

        assert_eq!(
            terminal_mouse_scroll_report_bytes(
                1,
                2,
                -2,
                gpui::Modifiers {
                    shift: true,
                    ..Default::default()
                },
                mode,
            ),
            Some(b"\x1b[<69;2;3M\x1b[<69;2;3M".to_vec())
        );
    }

    #[test]
    fn terminal_grid_detects_application_cursor_mode() {
        let mut grid = TerminalGrid::new(TerminalSize::new(12, 3));
        assert!(!grid.uses_app_cursor());

        grid.feed(b"\x1b[?1h");
        assert!(grid.uses_app_cursor());

        grid.feed(b"\x1b[?1l");
        assert!(!grid.uses_app_cursor());
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
            DEFAULT_TERMINAL_PADDING,
            DEFAULT_SIDEBAR_WIDTH,
            TileLayout::Grid,
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
            DEFAULT_TERMINAL_PADDING,
            DEFAULT_SIDEBAR_WIDTH,
            TileLayout::Grid,
            1,
            false,
        );

        assert!(size.columns >= 20);
        assert_eq!(size.rows, 8);
    }

    #[test]
    fn terminal_size_tracks_font_size() {
        let default_font = terminal_size_for_viewport(
            Size {
                width: px(1180.0),
                height: px(760.0),
            },
            12.0,
            DEFAULT_TERMINAL_PADDING,
            DEFAULT_SIDEBAR_WIDTH,
            TileLayout::Grid,
            1,
            false,
        );
        let larger_font = terminal_size_for_viewport(
            Size {
                width: px(1180.0),
                height: px(760.0),
            },
            16.0,
            DEFAULT_TERMINAL_PADDING,
            DEFAULT_SIDEBAR_WIDTH,
            TileLayout::Grid,
            1,
            false,
        );

        assert!(larger_font.columns < default_font.columns);
        assert!(larger_font.rows < default_font.rows);
    }

    #[test]
    fn terminal_size_tracks_padding() {
        let compact = terminal_size_for_viewport(
            Size {
                width: px(1180.0),
                height: px(760.0),
            },
            12.0,
            8.0,
            DEFAULT_SIDEBAR_WIDTH,
            TileLayout::Grid,
            1,
            false,
        );
        let roomy = terminal_size_for_viewport(
            Size {
                width: px(1180.0),
                height: px(760.0),
            },
            12.0,
            24.0,
            DEFAULT_SIDEBAR_WIDTH,
            TileLayout::Grid,
            1,
            false,
        );

        assert!(roomy.columns < compact.columns);
        assert!(roomy.rows < compact.rows);
    }

    fn test_state_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("lazyterm-{name}-{suffix}"))
    }
}
