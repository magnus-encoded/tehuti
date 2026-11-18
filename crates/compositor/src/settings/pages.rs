//! The Display and Input pages: rows of equal square tiles.

use super::{
    AMBER, CYAN, WHITE,
    canvas::Canvas,
    tiles::{DISPLAY_SIDE, Grid, wrap},
};
use tehuti_core::{
    dispatch::{
        CONTROLLER_ROW, CONTROLLER_SETTINGS_ROW, Connection, MICROPHONE_ROW, MicKind, MicrophoneChoice,
        PadKind, Settings, TALK_OPTIONS,
    },
    tiles::{Focus, clamp},
};

const FACE: [u8; 4] = [36, 26, 14, 255];
const FACE_FOCUSED: [u8; 4] = [65, 43, 12, 255];
const EDGE: [u8; 4] = [90, 70, 40, 255];
/// The corner radius of every tile, in reference pixels.
const TILE_RADIUS: usize = 14;
const DIM: [u8; 4] = [120, 110, 95, 255];
/// The origin line on a pad tile: quiet, but readable.
const ORIGIN_TEXT: [u8; 4] = [190, 175, 150, 255];
/// The outline of the pad whose settings row has focus: dimmer than the focus outline.
const SELECTED: [u8; 4] = [150, 128, 36, 255];
const SCREEN: [u8; 4] = [20, 14, 8, 255];

/// A tile's box in reference pixels: left, top, width, height.
type Area = (usize, usize, usize, usize);

impl Canvas {
    /// Draws the frame of tile (`row`, `col`) and returns its corner, or none when the row lacks it.
    fn tile_frame(
        &mut self,
        grid: &Grid,
        cell: (usize, usize),
        focus: Focus,
        marked: bool,
    ) -> Option<(usize, usize)> {
        let (x, y) = grid.origin(cell.0, cell.1)?;
        let side = grid.side();
        let focused = focus.row == cell.0 && focus.col == cell.1;
        let edge = if focused { CYAN } else { EDGE };
        let thickness = if focused { 4 } else { 2 };
        self.rounded_rectangle((x, y, side, side), TILE_RADIUS, edge);
        self.rounded_rectangle(
            (
                x.saturating_add(thickness),
                y.saturating_add(thickness),
                side.saturating_sub(thickness.saturating_mul(2)),
                side.saturating_sub(thickness.saturating_mul(2)),
            ),
            TILE_RADIUS.saturating_sub(thickness),
            if focused { FACE_FOCUSED } else { FACE },
        );
        if marked {
            self.rectangle(
                x.saturating_add(side).saturating_sub(20),
                y.saturating_add(8),
                10,
                10,
                CYAN,
            );
        }
        Some((x, y))
    }

    /// Centres up to three wrapped lines in `area`.
    fn tile_text(&mut self, area: Area, lines: &[String], points: usize, colour: [u8; 4]) {
        let (left, top, width, height) = area;
        let per_line = width
            .saturating_sub(12)
            .saturating_mul(10)
            .checked_div(points.saturating_mul(6))
            .unwrap_or(1);
        let wrapped: Vec<String> = lines
            .iter()
            .flat_map(|line| wrap(line, per_line, 3))
            .take(3)
            .collect();
        let line_height = points.saturating_mul(6).checked_div(5).unwrap_or(points);
        let last = wrapped.len().saturating_sub(1);
        let middle = (
            left.saturating_mul(2).saturating_add(width),
            top.saturating_mul(2).saturating_add(height),
        );
        for (index, line) in wrapped.iter().enumerate() {
            let twice = index.saturating_mul(2);
            let offset = twice.abs_diff(last).saturating_mul(line_height);
            let above = twice < last;
            let centre_y = if above {
                middle.1.saturating_sub(offset)
            } else {
                middle.1.saturating_add(offset)
            };
            self.centred((middle.0, centre_y), line, points, colour);
        }
    }

    pub(super) fn display(&mut self, settings: &Settings) {
        let rows = settings.display_rows();
        let focus = clamp(&rows, settings.display_focus);
        let grid = Grid::new(&rows, 0, focus, Some(DISPLAY_SIDE));
        self.resolution_row(settings, &grid, focus);
        self.refresh_row(settings, &grid, focus);
        for (col, step) in settings.scale_steps().into_iter().enumerate() {
            let marked = step == settings.scale.get();
            if let Some(corner) = self.tile_frame(&grid, (2, col), focus, marked) {
                self.scale_tile(corner, grid.side(), step);
            }
        }
        for (col, font) in tehuti_core::bar::Font::ALL.into_iter().enumerate() {
            let marked = font == settings.bar_font;
            if let Some(corner) = self.tile_frame(&grid, (3, col), focus, marked) {
                self.font_tile(corner, grid.side(), font);
            }
        }
        if settings.modes.is_empty() {
            self.text(420, 190, "NO DRIVER MODES AVAILABLE", 2, AMBER);
        }
    }

    fn resolution_row(&mut self, settings: &Settings, grid: &Grid, focus: Focus) {
        let side = grid.side();
        let points = side.checked_div(10).unwrap_or(8).max(8);
        let active = settings.modes.get(settings.current);
        let active_resolution = active.and_then(|mode| mode.split('@').next());
        for (col, name) in settings.resolutions().iter().enumerate() {
            let marked = Some(name.as_str()) == active_resolution;
            if let Some((x, y)) = self.tile_frame(grid, (0, col), focus, marked) {
                let (width, height) = name.split_once('x').unwrap_or((name.as_str(), ""));
                if let (Ok(width), Ok(height)) = (width.parse::<usize>(), height.parse::<usize>()) {
                    self.shape((x, y), side, (width, height));
                }
                let numbers = format!("{width} X {height}");
                let below = y.saturating_add(side).saturating_sub(side / 5);
                self.tile_text((x, below, side, side / 5), &[numbers], points, WHITE);
            }
        }
    }

    /// An outline with the display's aspect ratio, as large as the tile's upper part allows.
    fn shape(&mut self, corner: (usize, usize), side: usize, ratio: (usize, usize)) {
        if ratio.0 == 0 || ratio.1 == 0 {
            return;
        }
        let room = (side.saturating_sub(side / 4), side.saturating_sub(side / 2));
        let width = room
            .0
            .min(room.1.saturating_mul(ratio.0).checked_div(ratio.1).unwrap_or(0));
        let height = room
            .1
            .min(room.0.saturating_mul(ratio.1).checked_div(ratio.0).unwrap_or(0));
        let left = corner.0.saturating_add(side.saturating_sub(width) / 2);
        let top = corner
            .1
            .saturating_add(side / 12)
            .saturating_add(room.1.saturating_sub(height) / 2);
        let thickness = 3;
        self.rectangle(left, top, width, thickness, CYAN);
        self.rectangle(
            left,
            top.saturating_add(height).saturating_sub(thickness),
            width,
            thickness,
            CYAN,
        );
        self.rectangle(left, top, thickness, height, CYAN);
        self.rectangle(
            left.saturating_add(width).saturating_sub(thickness),
            top,
            thickness,
            height,
            CYAN,
        );
    }

    /// The refresh rates of the resolution that has focus.
    fn refresh_row(&mut self, settings: &Settings, grid: &Grid, focus: Focus) {
        let side = grid.side();
        let points = side.checked_div(10).unwrap_or(8).max(8);
        let modes = settings.refresh_modes(settings.display_resolution);
        for (col, mode) in modes.into_iter().enumerate() {
            if let Some((x, y)) = self.tile_frame(grid, (1, col), focus, mode == settings.current) {
                let rate = settings
                    .modes
                    .get(mode)
                    .and_then(|name| name.split_once('@'))
                    .map_or("", |(_, rate)| rate);
                if let Ok(hertz) = rate.parse::<f32>() {
                    let width = side.saturating_sub(side / 4);
                    let left = x.saturating_add(side.saturating_sub(width) / 2);
                    let middle = y.saturating_add(side.saturating_mul(2) / 5);
                    self.wave_across((left, middle), width, hertz);
                }
                let below = y.saturating_add(side).saturating_sub(side / 5);
                self.tile_text((x, below, side, side / 5), &[format!("{rate} HZ")], points, WHITE);
            }
        }
    }

    /// A small screen with the strip of it that a bar at this scale covers, and the scale.
    fn scale_tile(&mut self, corner: (usize, usize), side: usize, step: u32) {
        let (x, y) = corner;
        let inset = side.checked_div(6).unwrap_or(0);
        let inner = side.saturating_sub(inset.saturating_mul(2));
        let screen_height = inner.saturating_mul(3).checked_div(5).unwrap_or(0);
        let left = x.saturating_add(inset);
        let top = y.saturating_add(inset);
        self.rectangle(left, top, inner, screen_height, SCREEN);
        let share = usize::try_from(step).unwrap_or(usize::MAX);
        let strip = screen_height
            .saturating_mul(share)
            .checked_div(10)
            .unwrap_or(0)
            .clamp(2, screen_height.max(2));
        self.rectangle(
            left,
            top.saturating_add(screen_height).saturating_sub(strip),
            inner,
            strip,
            CYAN,
        );
        let below = top.saturating_add(screen_height);
        let remaining = y.saturating_add(side).saturating_sub(below);
        let points = side.checked_div(7).unwrap_or(8).max(8);
        self.tile_text((x, below, side, remaining), &[format!("{step}X")], points, WHITE);
    }

    /// One large sample, drawn by the font the tile stands for at its physical size, and the font's name.
    fn font_tile(&mut self, corner: (usize, usize), side: usize, font: tehuti_core::bar::Font) {
        let (x, y) = corner;
        let middle = (
            x.saturating_add(side / 2),
            y.saturating_add(side.saturating_mul(2) / 5),
        );
        self.sample_text(middle, "Aa", font, side / 2, WHITE);
        let points = side.checked_div(10).unwrap_or(8).max(8);
        let below = y.saturating_add(side).saturating_sub(side / 5);
        self.tile_text(
            (x, below, side, side / 5),
            &[font.label().to_uppercase()],
            points,
            CYAN,
        );
    }

    pub(super) fn input(&mut self, settings: &Settings, speech: &str) {
        let rows = settings.input.rows();
        let focus = clamp(&rows, settings.input.focus);
        let grid = Grid::input(&rows, focus);
        self.controller_row(settings, &grid, focus);
        self.controller_settings_row(settings, &grid, focus);
        self.microphone_row(settings, &grid, focus);
        self.text(760, 58, "SPEECH RUNS ON", 2, WHITE);
        let speech: String = speech.chars().take(26).collect();
        self.text(
            760,
            88,
            &speech,
            3,
            if speech.starts_with("CPU") { AMBER } else { CYAN },
        );
    }

    fn pad_drawing(&mut self, corner: (usize, usize), side: usize, kind: PadKind, colour: [u8; 4]) {
        let icon = match kind {
            PadKind::Xbox => "xbox",
            PadKind::PlayStation => "dualsense",
            PadKind::Switch => "switch",
            PadKind::Generic => "generic-pad",
        };
        self.input_icon(corner, side, icon, colour);
    }

    /// The origin glyph in a tile's corner; nothing when the source is unknown.
    fn connection_glyph(
        &mut self,
        corner: (usize, usize),
        side: usize,
        connection: Connection,
        colour: [u8; 4],
    ) {
        if let Some(icon) = connection.glyph() {
            self.svg_icon(corner, side.saturating_mul(2) / 7, icon, colour);
        }
    }

    /// Artwork centred in the upper part of a tile, leaving the bottom for words.
    fn input_icon(&mut self, corner: (usize, usize), side: usize, icon: &'static str, colour: [u8; 4]) {
        let art = side.saturating_mul(4) / 7;
        self.svg_icon(
            (
                corner.0.saturating_add(side.saturating_sub(art) / 2),
                corner.1.saturating_add(side / 6),
            ),
            art,
            icon,
            colour,
        );
    }

    /// One small line centred on the bottom of a tile.
    fn tile_foot(&mut self, corner: (usize, usize), side: usize, text: &str, points: usize, colour: [u8; 4]) {
        let middle = (
            corner.0.saturating_mul(2).saturating_add(side),
            corner
                .1
                .saturating_add(side)
                .saturating_sub(side / 12)
                .saturating_mul(2),
        );
        self.centred(middle, text, points, colour);
    }

    /// A thin outline around a tile, for the pad that the focused settings row edits.
    fn selected_outline(&mut self, corner: (usize, usize), side: usize) {
        let (x, y) = corner;
        self.rounded_rectangle((x, y, side, side), TILE_RADIUS, SELECTED);
        self.rounded_rectangle(
            (
                x.saturating_add(3),
                y.saturating_add(3),
                side.saturating_sub(6),
                side.saturating_sub(6),
            ),
            TILE_RADIUS.saturating_sub(3),
            FACE,
        );
    }

    fn mic_drawing(&mut self, corner: (usize, usize), side: usize, kind: MicKind, colour: [u8; 4]) {
        let icon = match kind {
            MicKind::Headset => "headset",
            MicKind::Usb => "usb-mic",
            MicKind::Builtin => "builtin",
            MicKind::Network => "network",
            MicKind::Phone => "phone",
            MicKind::Generic => "mic",
        };
        self.input_icon(corner, side, icon, colour);
    }

    fn controller_row(&mut self, settings: &Settings, grid: &Grid, focus: Focus) {
        let input = &settings.input;
        let side = grid.side();
        let points = side.checked_div(14).unwrap_or(8).max(8);
        for (col, controller) in input.controllers.iter().enumerate() {
            let enabled = input.controller_enabled(&controller.key);
            let Some((x, y)) = self.tile_frame(grid, (CONTROLLER_ROW, col), focus, enabled) else {
                continue;
            };
            let asking = input.pending_disable.as_deref() == Some(controller.key.as_str());
            let colour = if asking {
                AMBER
            } else if enabled {
                WHITE
            } else {
                DIM
            };
            if focus.row == CONTROLLER_SETTINGS_ROW && input.controller_cursor == col {
                self.selected_outline((x, y), side);
            }
            self.pad_drawing((x, y), side, controller.kind(), colour);
            self.tile_text(
                (x, y.saturating_add(side.saturating_mul(2) / 3), side, side / 4),
                core::slice::from_ref(&controller.label),
                points,
                colour,
            );
            self.tile_foot(
                (x, y),
                side,
                controller.connection.caption(),
                points.max(8),
                if asking { AMBER } else { ORIGIN_TEXT },
            );
            self.connection_glyph(
                (x.saturating_add(8), y.saturating_add(8)),
                side,
                controller.connection,
                colour,
            );
            if input.controller_in_use(&controller.key) {
                self.rectangle(
                    x.saturating_add(side).saturating_sub(40),
                    y.saturating_add(8),
                    10,
                    10,
                    AMBER,
                );
            }
        }
    }

    /// The open controller's own settings: ENABLED, then its push-to-talk button.
    fn controller_settings_row(&mut self, settings: &Settings, grid: &Grid, focus: Focus) {
        let input = &settings.input;
        let Some(controller) = input.selected_controller() else {
            return;
        };
        let side = grid.side();
        let points = side.checked_div(10).unwrap_or(8).max(8);
        let enabled = input.controller_enabled(&controller.key);
        if let Some((x, y)) = grid.origin(CONTROLLER_SETTINGS_ROW, 0) {
            let heading = controller.spoken_name().to_uppercase();
            self.text(x, y.saturating_sub(28), &heading, 2, AMBER);
        }
        if let Some((x, y)) = self.tile_frame(grid, (CONTROLLER_SETTINGS_ROW, 0), focus, enabled) {
            let colour = if enabled { WHITE } else { DIM };
            self.input_icon(
                (x, y),
                side,
                if enabled { "toggle-on" } else { "toggle-off" },
                colour,
            );
            self.tile_foot(
                (x, y),
                side,
                if enabled { "ENABLED" } else { "OFF" },
                points,
                colour,
            );
        }
        for (index, option) in TALK_OPTIONS.into_iter().enumerate() {
            let col = index.saturating_add(1);
            let marked = input.talk_choice() == option;
            if let Some((x, y)) = self.tile_frame(grid, (CONTROLLER_SETTINGS_ROW, col), focus, marked) {
                let icon = match option {
                    None => "button-view",
                    Some(tehuti_protocol::ButtonName::Record) => "button-share",
                    Some(_) => "button-menu",
                };
                let label = tehuti_core::dispatch::InputSettings::talk_label(option);
                self.input_icon((x, y), side, icon, WHITE);
                self.tile_foot((x, y), side, &format!("TALK {label}"), points, WHITE);
            }
        }
        let deadzone = (CONTROLLER_SETTINGS_ROW, TALK_OPTIONS.len().saturating_add(1));
        if let Some((x, y)) = self.tile_frame(grid, deadzone, focus, true) {
            let percent = input
                .deadzone(&controller.key)
                .checked_div(10)
                .unwrap_or_default();
            self.input_icon((x, y), side, "deadzone", WHITE);
            self.tile_foot((x, y), side, &format!("DEADZONE {percent}%"), points, WHITE);
        }
        let recentre = (CONTROLLER_SETTINGS_ROW, TALK_OPTIONS.len().saturating_add(2));
        let measured = input.controller_recentres.contains_key(&controller.key);
        if let Some((x, y)) = self.tile_frame(grid, recentre, focus, measured) {
            let words = if measured { "RECENTRE SET" } else { "RECENTRE" };
            self.input_icon((x, y), side, "recentre", WHITE);
            self.tile_foot((x, y), side, words, points, WHITE);
        }
    }

    fn microphone_row(&mut self, settings: &Settings, grid: &Grid, focus: Focus) {
        let input = &settings.input;
        let side = grid.side();
        let points = side.checked_div(14).unwrap_or(8).max(8);
        for (col, choice) in input.microphone_options().iter().enumerate() {
            let marked = *choice == input.microphone;
            if let Some((x, y)) = self.tile_frame(grid, (MICROPHONE_ROW, col), focus, marked) {
                let label = input.microphone_tile_label(choice);
                let microphone = match choice {
                    MicrophoneChoice::Named(node) => {
                        input.microphones.iter().find(|known| known.name == *node)
                    }
                    MicrophoneChoice::All | MicrophoneChoice::Auto => None,
                };
                let enabled = match choice {
                    MicrophoneChoice::Named(node) => input.microphone_enabled(node),
                    MicrophoneChoice::All | MicrophoneChoice::Auto => true,
                };
                let colour = if enabled { WHITE } else { DIM };
                match (choice, microphone) {
                    (MicrophoneChoice::All, _) => self.input_icon((x, y), side, "all", colour),
                    (MicrophoneChoice::Auto, _) => self.input_icon((x, y), side, "auto", colour),
                    (MicrophoneChoice::Named(_), Some(known)) => {
                        self.mic_drawing((x, y), side, known.kind(), colour);
                    }
                    (MicrophoneChoice::Named(_), None) => {
                        self.mic_drawing((x, y), side, MicKind::Generic, DIM);
                    }
                }
                let words = if enabled { label } else { format!("{label} OFF") };
                let below = y.saturating_add(side.saturating_mul(7) / 10);
                self.tile_text((x, below, side, side / 4), &[words], points, colour);
            }
        }
    }
}
