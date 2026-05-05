use lazyterm_api::{TerminalRail as ApiTerminalRail, TileLayout as ApiTileLayout};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

pub(super) const COMPACT_SIDEBAR_WIDTH: f32 = 60.0;
pub(super) const DEFAULT_SIDEBAR_WIDTH: f32 = 212.0;
pub(super) const WIDE_SIDEBAR_WIDTH: f32 = 268.0;
pub(super) const MIN_CUSTOM_SIDEBAR_WIDTH: f32 = 52.0;
pub(super) const MAX_CUSTOM_SIDEBAR_WIDTH: f32 = 288.0;
pub(super) const DEFAULT_TERMINAL_PADDING: f32 = 16.0;
pub(super) const DEFAULT_SPLIT_RATIO: f32 = 0.5;
pub(super) const MIN_SPLIT_RATIO: f32 = 0.2;
pub(super) const MAX_SPLIT_RATIO: f32 = 0.8;
pub(super) const MIN_TERMINAL_FONT_SIZE: f32 = 8.0;
pub(super) const MAX_TERMINAL_FONT_SIZE: f32 = 24.0;
pub(super) const DEFAULT_TERMINAL_FONT_FAMILY: &str = "JetBrains Mono";

#[derive(Clone, Debug)]
pub(super) struct UiSettings {
    pub(super) tile_sessions: bool,
    pub(super) tile_layout: TileLayout,
    pub(super) rail_width: RailWidth,
    pub(super) custom_rail_width: Option<f32>,
    pub(super) terminal_font_family: String,
    pub(super) terminal_font_size: f32,
    pub(super) terminal_padding: f32,
    pub(super) split_ratio: f32,
    pub(super) pane_ratios: Vec<f32>,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            tile_sessions: false,
            tile_layout: TileLayout::Grid,
            rail_width: RailWidth::Compact,
            custom_rail_width: None,
            terminal_font_family: DEFAULT_TERMINAL_FONT_FAMILY.to_string(),
            terminal_font_size: 12.0,
            terminal_padding: DEFAULT_TERMINAL_PADDING,
            split_ratio: DEFAULT_SPLIT_RATIO,
            pane_ratios: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(super) enum TileLayout {
    Grid,
    Columns,
    Rows,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(super) enum RailWidth {
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
            custom_rail_width: value
                .custom_rail_width
                .map(|width| width.clamp(MIN_CUSTOM_SIDEBAR_WIDTH, MAX_CUSTOM_SIDEBAR_WIDTH)),
            terminal_font_family: normalize_font_family(value.terminal_font_family),
            terminal_font_size: value
                .terminal_font_size
                .clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE),
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
            custom_rail_width: value.custom_rail_width,
            terminal_font_family: value.terminal_font_family,
            terminal_font_size: value.terminal_font_size,
            terminal_padding: value.terminal_padding,
            split_ratio: value.split_ratio,
            pane_ratios: value.pane_ratios,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PersistedUiSettings {
    tile_sessions: bool,
    #[serde(default = "default_tile_layout")]
    tile_layout: TileLayout,
    #[serde(default = "default_rail_width")]
    rail_width: RailWidth,
    #[serde(default)]
    custom_rail_width: Option<f32>,
    #[serde(default = "default_terminal_font_family")]
    terminal_font_family: String,
    terminal_font_size: f32,
    #[serde(default = "default_terminal_padding")]
    terminal_padding: f32,
    #[serde(default = "default_split_ratio")]
    split_ratio: f32,
    #[serde(default)]
    pane_ratios: Vec<f32>,
}

fn default_tile_layout() -> TileLayout {
    TileLayout::Grid
}

fn default_rail_width() -> RailWidth {
    RailWidth::Compact
}

fn default_terminal_padding() -> f32 {
    DEFAULT_TERMINAL_PADDING
}

fn default_split_ratio() -> f32 {
    DEFAULT_SPLIT_RATIO
}

fn default_terminal_font_family() -> String {
    DEFAULT_TERMINAL_FONT_FAMILY.to_string()
}

pub(super) fn normalize_font_family(family: String) -> String {
    let mut family = family
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .to_string();
    family.truncate(80);

    if family.is_empty() {
        default_terminal_font_family()
    } else {
        family
    }
}

pub(super) fn sidebar_width_for_rail(width: RailWidth) -> f32 {
    match width {
        RailWidth::Compact => COMPACT_SIDEBAR_WIDTH,
        RailWidth::Default => DEFAULT_SIDEBAR_WIDTH,
        RailWidth::Wide => WIDE_SIDEBAR_WIDTH,
    }
}

pub(super) fn load_ui_settings(state_dir: &Path) -> Option<UiSettings> {
    let path = state_dir.join("ui-settings.json");
    let payload = fs::read_to_string(path).ok()?;
    serde_json::from_str::<PersistedUiSettings>(&payload)
        .ok()
        .map(Into::into)
}

pub(super) fn save_ui_settings(
    state_dir: &Path,
    settings: UiSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(state_dir)?;
    let payload = serde_json::to_string_pretty(&PersistedUiSettings::from(settings))?;
    fs::write(state_dir.join("ui-settings.json"), payload)?;
    Ok(())
}
