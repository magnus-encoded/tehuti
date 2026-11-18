//! The Display and Input pages: rows of equal square tiles.

use super::{
    AMBER, CYAN, WHITE,
    canvas::Canvas,
    tiles::{DISPLAY_SIDE, Grid, wrap},
};
use tehuti_core::{
    dispatch::{Microphone, MicrophoneChoice, Settings, TALK_OPTIONS},
    tiles::{Focus, clamp},
};

const FACE: [u8; 4] = [36, 26, 14, 255];
const FACE_FOCUSED: [u8; 4] = [65, 43, 12, 255];
const EDGE: [u8; 4] = [90, 70, 40, 255];
const DIM: [u8; 4] = [120, 110, 95, 255];
const SCREEN: [u8; 4] = [20, 14, 8, 255];
const GLYPH_WIDTH: usize = 16;
const GLYPH_HEIGHT: usize = 24;
/// Each sample of the font tile's large letters is drawn as a block this wide.
const MAGNIFY: usize = 4;
/// Height kept under each Input tile for its name.
const CAPTION: usize = 22;
const CAPTION_POINTS: usize = 13;
const SAMPLE: [char; 2] = ['A', 'a'];

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
        let line: String = name.to_uppercase().chars().take(per_line).collect();
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
            self.text(96, 606, "NO DRIVER MODES AVAILABLE", 3, AMBER);
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
    fn shape(&mut self, corner: (usize, usize), side: usize, size: (usize, usize)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        let room = (side.saturating_sub(side / 4), side.saturating_sub(side / 2));
        let width = room.0.min(room.1.saturating_mul(size.0).checked_div(size.1).unwrap_or(0));
        let height = room.1.min(room.0.saturating_mul(size.1).checked_div(size.0).unwrap_or(0));
        let left = corner.0.saturating_add(side.saturating_sub(width) / 2);
        let top = corner.1.saturating_add(side / 12).saturating_add(room.1.saturating_sub(height) / 2);
        let thickness = 3;
        self.rectangle(left, top, width, thickness, CYAN);
        self.rectangle(left, top.saturating_add(height).saturating_sub(thickness), width, thickness, CYAN);
        self.rectangle(left, top, thickness, height, CYAN);
        self.rectangle(left.saturating_add(width).saturating_sub(thickness), top, thickness, height, CYAN);
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
                    self.frequency((x, y), side, hertz);
                }
                let below = y.saturating_add(side).saturating_sub(side / 5);
                self.tile_text((x, below, side, side / 5), &[format!("{rate} HZ")], points, WHITE);
            }
        }
    }

    /// A sine wave across the tile with a cycle for about every 15 Hz.
    fn frequency(&mut self, corner: (usize, usize), side: usize, hertz: f32) {
        let width = side.saturating_sub(side / 4);
        let left = corner.0.saturating_add(side.saturating_sub(width) / 2);
        let middle = corner.1.saturating_add(side * 2 / 5);
        let amplitude = (side as f32) / 7.0;
        let cycles = (hertz / 15.0).clamp(1.0, 40.0);
        for column in 0..width {
            let phase = column as f32 / width as f32 * cycles * core::f32::consts::TAU;
            let offset = phase.sin() * amplitude;
            self.rectangle(
                left.saturating_add(column),
                (middle as f32 + offset).max(0.0) as usize,
                2,
                3,
                CYAN,
            );
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

    /// One large sample, drawn by the font the tile stands for, and the font's name.
    fn font_tile(&mut self, corner: (usize, usize), side: usize, font: tehuti_core::bar::Font) {
        let (x, y) = corner;
        let span = GLYPH_WIDTH.saturating_mul(MAGNIFY);
        let left = x.saturating_add(side.saturating_sub(span.saturating_mul(SAMPLE.len())) / 2);
        let top = y.saturating_add(side / 12);
        for (index, character) in SAMPLE.into_iter().enumerate() {
            let samples = crate::bar_font::coverage(character, font, true);
            for (cell, alpha) in samples.iter().enumerate() {
                if *alpha < 32 {
                    continue;
                }
                let column = cell.checked_rem(GLYPH_WIDTH).unwrap_or(0);
                let row = cell.checked_div(GLYPH_WIDTH).unwrap_or(0);
                let shade = |face: u8, ink: u8| {
                    let mixed = u16::from(face) * u16::from(255 - *alpha) + u16::from(ink) * u16::from(*alpha);
                    u8::try_from(mixed / 255).unwrap_or(u8::MAX)
                };
                let colour = [
                    shade(FACE[0], WHITE[0]),
                    shade(FACE[1], WHITE[1]),
                    shade(FACE[2], WHITE[2]),
                    255,
                ];
                self.rectangle(
                    left.saturating_add(index.saturating_mul(span))
                        .saturating_add(column.saturating_mul(MAGNIFY)),
                    top.saturating_add(row.saturating_mul(MAGNIFY)),
                    MAGNIFY,
                    MAGNIFY,
                    colour,
                );
            }
        }
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
        let grid = Grid::new(&rows, CAPTION, focus, None);
        self.controller_row(settings, &grid, focus);
        self.microphone_row(settings, &grid, focus);
        self.talk_row(settings, &grid, focus);
        self.text(96, 630, "SPEECH RUNS ON", 2, WHITE);
        let speech: String = speech.chars().take(36).collect();
        self.text(
            360,
            626,
            &speech,
            3,
            if speech.starts_with("CPU") { AMBER } else { CYAN },
        );
    }

    fn controller_row(&mut self, settings: &Settings, grid: &Grid, focus: Focus) {
        let input = &settings.input;
        let side = grid.side();
        let points = side.checked_div(7).unwrap_or(8).max(8);
        for (col, controller) in input.controllers.iter().enumerate() {
            let enabled = input.controller_enabled(&controller.name);
            let Some((x, y)) = self.tile_frame(grid, (0, col), focus, enabled) else {
                continue;
            };
            let asking = input.pending_disable.as_deref() == Some(controller.name.as_str());
            let colour = if asking {
                AMBER
            } else if enabled {
                WHITE
            } else {
                DIM
            };
            self.tile_text(
                (x, y, side, side),
                core::slice::from_ref(&controller.label),
                points,
                colour,
            );
            self.caption((x, y), side, &controller.spoken_name(), colour);
            if input.controller_in_use(&controller.name) {
                let bottom = y.saturating_add(side).saturating_sub(18);
                self.rectangle(x.saturating_add(8), bottom, 10, 10, AMBER);
            }
        }
    }

    fn microphone_row(&mut self, settings: &Settings, grid: &Grid, focus: Focus) {
        let input = &settings.input;
        let side = grid.side();
        let points = side.checked_div(7).unwrap_or(8).max(8);
        for (col, choice) in input.microphone_options().iter().enumerate() {
            let marked = *choice == input.microphone;
            if let Some((x, y)) = self.tile_frame(grid, (1, col), focus, marked) {
                let label = input.microphone_tile_label(choice);
                let name = match choice {
                    MicrophoneChoice::Named(node) => input
                        .microphones
                        .iter()
                        .find(|microphone| microphone.name == *node)
                        .map(Microphone::spoken_name),
                    MicrophoneChoice::All | MicrophoneChoice::Auto => None,
                };
                let enabled = match choice {
                    MicrophoneChoice::Named(node) => input.microphone_enabled(node),
                    MicrophoneChoice::All | MicrophoneChoice::Auto => true,
                };
                let lines = if enabled {
                    vec![label]
                } else {
                    vec![label, "OFF".to_owned()]
                };
                let colour = if enabled { WHITE } else { DIM };
                self.tile_text((x, y, side, side), &lines, points, colour);
                if let Some(name) = name {
                    self.caption((x, y), side, &name, colour);
                }
            }
        }
    }

    fn talk_row(&mut self, settings: &Settings, grid: &Grid, focus: Focus) {
        let input = &settings.input;
        let side = grid.side();
        let points = side.checked_div(7).unwrap_or(8).max(8);
        let binding = input.binding_controller().is_some();
        for (col, option) in TALK_OPTIONS.into_iter().enumerate() {
            let marked = binding && input.talk_choice() == option;
            if let Some((x, y)) = self.tile_frame(grid, (2, col), focus, marked) {
                let label = tehuti_core::dispatch::InputSettings::talk_label(option);
                let lines = ["TALK".to_owned(), label.to_owned()];
                self.tile_text((x, y, side, side), &lines, points, WHITE);
            }
        }
    }
}
