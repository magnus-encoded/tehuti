//! The Display and Input pages: rows of equal square tiles.

use super::{
    AMBER, CYAN, WHITE,
    canvas::Canvas,
    tiles::{DISPLAY_SIDE, Grid, INPUT_SIDE, wrap},
};
use tehuti_core::{
    dispatch::{
        CONTROLLER_ROW, CONTROLLER_SETTINGS_ROW, Connection, MICROPHONE_ROW, MicKind, MicrophoneChoice,
        PadKind, REMOTE_ROW, Settings, TALK_OPTIONS,
    },
    tiles::{Focus, clamp},
};

const FACE: [u8; 4] = [36, 26, 14, 255];
const FACE_FOCUSED: [u8; 4] = [65, 43, 12, 255];
const EDGE: [u8; 4] = [90, 70, 40, 255];
const DIM: [u8; 4] = [120, 110, 95, 255];
const SCREEN: [u8; 4] = [20, 14, 8, 255];
/// Height kept under each Input tile for its name.
const CAPTION: usize = 22;
const CAPTION_POINTS: usize = 13;

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
        self.rectangle(x, y, side, side, if focused { FACE_FOCUSED } else { FACE });
        let edge = if focused { CYAN } else { EDGE };
        let thickness = if focused { 4 } else { 2 };
        self.rectangle(x, y, side, thickness, edge);
        self.rectangle(
            x,
            y.saturating_add(side).saturating_sub(thickness),
            side,
            thickness,
            edge,
        );
        self.rectangle(x, y, thickness, side, edge);
        self.rectangle(
            x.saturating_add(side).saturating_sub(thickness),
            y,
            thickness,
            side,
            edge,
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

    /// The short name under a tile: one line, cut to the tile's width.
    fn caption(&mut self, corner: (usize, usize), side: usize, name: &str, colour: [u8; 4]) {
        let per_line = side
            .saturating_mul(10)
            .checked_div(CAPTION_POINTS.saturating_mul(6))
            .unwrap_or(1);
        let upper = name.to_uppercase();
        let line = if upper.chars().count() <= per_line {
            upper
        } else {
            // Keep the last word: it is what tells identical pads apart.
            let tail = upper.rsplit(' ').next().unwrap_or("");
            let head: String = upper
                .chars()
                .take(per_line.saturating_sub(tail.chars().count()).saturating_sub(3))
                .collect();
            format!("{}.. {tail}", head.trim_end())
        };
        let middle = (
            corner.0.saturating_mul(2).saturating_add(side),
            corner
                .1
                .saturating_mul(2)
                .saturating_add(side.saturating_mul(2))
                .saturating_add(CAPTION),
        );
        self.centred(middle, &line, CAPTION_POINTS, colour);
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
        let grid = Grid::new(&rows, CAPTION, focus, Some(INPUT_SIDE));
        self.controller_row(settings, &grid, focus);
        self.controller_settings_row(settings, &grid, focus);
        self.microphone_row(settings, &grid, focus);
        self.remote_row(settings, &grid, focus);
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

    /// Draws `parts`, boxes in 24ths of the tile side, from the tile's corner.
    fn drawing(&mut self, corner: (usize, usize), side: usize, parts: &[Part], colour: [u8; 4]) {
        let unit = |value: usize| value.saturating_mul(side).checked_div(24).unwrap_or(0);
        for (left, top, width, height) in parts {
            self.rectangle(
                corner.0.saturating_add(unit(*left)),
                corner.1.saturating_add(unit(*top)),
                unit(*width).max(1),
                unit(*height).max(1),
                colour,
            );
        }
    }

    fn pad_drawing(&mut self, corner: (usize, usize), side: usize, kind: PadKind, colour: [u8; 4]) {
        let parts: &[Part] = match kind {
            PadKind::Xbox => &XBOX,
            PadKind::PlayStation => &PLAYSTATION,
            PadKind::Switch => &SWITCH,
            PadKind::Generic => &GENERIC_PAD,
        };
        self.drawing(corner, side, parts, colour);
        self.drawing(corner, side, pad_details(kind), FACE);
    }

    /// A small picture of how the pad connects, in a tile's corner.
    fn connection_glyph(
        &mut self,
        corner: (usize, usize),
        side: usize,
        connection: Connection,
        colour: [u8; 4],
    ) {
        let art: &[&str] = match connection {
            Connection::Usb => &[".#####.", ".#.#.#.", ".#####.", "...#...", "...#..."],
            Connection::Bluetooth => &[
                "..#..", "..##.", "#.#.#", ".###.", "..#..", ".###.", "#.#.#", "..##.", "..#..",
            ],
            Connection::Dongle => &["#.#.....", ".#.#####", ".#.#####", "#.#....."],
        };
        let cell = side.checked_div(45).unwrap_or(2).max(2);
        for (row, line) in art.iter().enumerate() {
            for (column, mark) in line.chars().enumerate() {
                if mark == '#' {
                    self.rectangle(
                        corner.0.saturating_add(column.saturating_mul(cell)),
                        corner.1.saturating_add(row.saturating_mul(cell)),
                        cell,
                        cell,
                        colour,
                    );
                }
            }
        }
    }

    fn mic_drawing(&mut self, corner: (usize, usize), side: usize, kind: MicKind, colour: [u8; 4]) {
        let parts: &[Part] = match kind {
            MicKind::Headset => &HEADSET,
            MicKind::Usb => &USB_MIC,
            MicKind::Builtin => &CHIP,
            MicKind::Network => &NETWORK,
            MicKind::Phone => &PHONE,
            MicKind::Generic => &CAPSULE_MIC,
        };
        self.drawing(corner, side, parts, colour);
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
            self.pad_drawing((x, y), side, controller.kind(), colour);
            let below = y.saturating_add(side).saturating_sub(side / 5);
            self.tile_text(
                (x, below, side, side / 5),
                core::slice::from_ref(&controller.label),
                points,
                colour,
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
        let points = side.checked_div(9).unwrap_or(8).max(8);
        let enabled = input.controller_enabled(&controller.key);
        if let Some((x, y)) = self.tile_frame(grid, (CONTROLLER_SETTINGS_ROW, 0), focus, enabled) {
            let word = if enabled { "ENABLED" } else { "OFF" };
            let colour = if enabled { WHITE } else { DIM };
            self.tile_text((x, y, side, side), &[word.to_owned()], points, colour);
            self.caption((x, y), side, &controller.spoken_name(), colour);
        }
        for (index, option) in TALK_OPTIONS.into_iter().enumerate() {
            let col = index.saturating_add(1);
            let marked = input.talk_choice() == option;
            if let Some((x, y)) = self.tile_frame(grid, (CONTROLLER_SETTINGS_ROW, col), focus, marked) {
                let label = tehuti_core::dispatch::InputSettings::talk_label(option);
                let lines = ["TALK".to_owned(), label.to_owned()];
                self.tile_text((x, y, side, side), &lines, points, WHITE);
            }
        }
    }

    /// The hosts that could send a remote microphone; confirm connects.
    fn remote_row(&mut self, settings: &Settings, grid: &Grid, focus: Focus) {
        let side = grid.side();
        let points = side.checked_div(14).unwrap_or(8).max(8);
        for (col, host) in settings.input.remote_hosts.iter().enumerate() {
            if let Some((x, y)) = self.tile_frame(grid, (REMOTE_ROW, col), focus, false) {
                let lines = ["ADD REMOTE".to_owned(), host.to_uppercase()];
                self.tile_text((x, y, side, side), &lines, points, WHITE);
            }
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
                    (MicrophoneChoice::All, _) => self.drawing((x, y), side, &ALL_MICS, colour),
                    (MicrophoneChoice::Auto, _) => self.drawing((x, y), side, &AUTO_MIC, colour),
                    (MicrophoneChoice::Named(_), Some(known)) => {
                        self.mic_drawing((x, y), side, known.kind(), colour);
                    }
                    (MicrophoneChoice::Named(_), None) => {
                        self.mic_drawing((x, y), side, MicKind::Generic, DIM);
                    }
                }
                let words = if enabled { label } else { format!("{label} OFF") };
                let below = y.saturating_add(side).saturating_sub(side / 5);
                self.tile_text((x, below, side, side / 5), &[words], points, colour);
                if let Some(known) = microphone {
                    self.caption((x, y), side, &known.caption(), colour);
                }
            }
        }
    }
}

/// A box in 24ths of a tile side: left, top, width, height.
type Part = (usize, usize, usize, usize);

const GENERIC_PAD: [Part; 3] = [(3, 5, 18, 9), (3, 12, 4, 5), (17, 12, 4, 5)];
const XBOX: [Part; 3] = [(3, 4, 18, 9), (3, 11, 5, 6), (16, 11, 5, 6)];
const PLAYSTATION: [Part; 4] = [(3, 5, 18, 8), (3, 12, 4, 6), (17, 12, 4, 6), (8, 3, 8, 1)];
const SWITCH: [Part; 2] = [(2, 3, 6, 14), (16, 3, 6, 14)];

/// The details cut into a pad's body: sticks, d-pad and buttons.
const fn pad_details(kind: PadKind) -> &'static [Part] {
    match kind {
        PadKind::Xbox => &[
            (5, 6, 3, 3),
            (9, 10, 4, 1),
            (10, 9, 2, 3),
            (14, 10, 3, 3),
            (16, 6, 1, 1),
            (18, 6, 1, 1),
            (17, 5, 1, 1),
            (17, 7, 1, 1),
        ],
        PadKind::PlayStation => &[
            (8, 5, 8, 3),
            (4, 7, 3, 1),
            (5, 6, 1, 3),
            (17, 7, 1, 1),
            (19, 7, 1, 1),
            (18, 6, 1, 1),
            (18, 8, 1, 1),
            (8, 10, 3, 3),
            (13, 10, 3, 3),
        ],
        PadKind::Switch => &[
            (4, 5, 3, 3),
            (4, 11, 1, 3),
            (3, 12, 3, 1),
            (17, 6, 1, 1),
            (19, 6, 1, 1),
            (18, 5, 1, 1),
            (18, 7, 1, 1),
            (17, 11, 3, 3),
        ],
        PadKind::Generic => &[(6, 7, 1, 3), (5, 8, 3, 1), (15, 7, 2, 2), (17, 9, 2, 2)],
    }
}

const CAPSULE_MIC: [Part; 6] = [
    (10, 2, 4, 8),
    (8, 8, 1, 3),
    (15, 8, 1, 3),
    (8, 11, 8, 1),
    (11, 12, 2, 3),
    (8, 15, 8, 1),
];
const USB_MIC: [Part; 4] = [(10, 2, 4, 9), (11, 11, 2, 4), (9, 15, 6, 3), (11, 18, 2, 1)];
const HEADSET: [Part; 7] = [
    (5, 3, 14, 1),
    (5, 3, 1, 7),
    (18, 3, 1, 7),
    (3, 8, 4, 7),
    (17, 8, 4, 7),
    (5, 14, 8, 1),
    (12, 13, 3, 2),
];
const CHIP: [Part; 7] = [
    (6, 4, 12, 11),
    (4, 6, 2, 1),
    (4, 9, 2, 1),
    (4, 12, 2, 1),
    (18, 6, 2, 1),
    (18, 9, 2, 1),
    (18, 12, 2, 1),
];
const NETWORK: [Part; 4] = [(10, 12, 4, 3), (8, 9, 8, 1), (5, 6, 14, 1), (2, 3, 20, 1)];
const PHONE: [Part; 3] = [(8, 2, 8, 15), (11, 15, 2, 1), (9, 3, 6, 10)];
const ALL_MICS: [Part; 3] = [(3, 5, 4, 8), (10, 3, 4, 10), (17, 5, 4, 8)];
const AUTO_MIC: [Part; 5] = [
    (10, 3, 4, 8),
    (5, 5, 1, 8),
    (18, 5, 1, 8),
    (5, 5, 3, 1),
    (16, 13, 3, 1),
];
