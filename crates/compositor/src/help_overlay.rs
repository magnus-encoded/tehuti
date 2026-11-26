//! Native-resolution "what can I say / press" overlay and its cached render element.

use crate::settings::canvas::Canvas;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    output::Output,
    utils::Transform,
};
use tehuti_core::{
    help_overlay::{GroupKind, HelpOverlay, Item, VISIBLE_ROWS},
    pad::PadFamily,
    ticker::{Direction, Piece, Rgb, Shape},
};

const WHITE: [u8; 4] = [240, 236, 229, 255];
const DIM: [u8; 4] = [151, 143, 132, 255];
const VOICE_HINT: [u8; 4] = [132, 198, 136, 255];
const EDGE: [u8; 4] = [98, 86, 72, 255];
const PANEL_BG: [u8; 4] = [30, 24, 20, 240];
const HEADER_BG: [u8; 4] = [53, 44, 37, 255];
const CARD_BG: [u8; 4] = [43, 36, 30, 255];
const CHIP_BG: [u8; 4] = [84, 80, 78, 255];
const KEY_BG: [u8; 4] = [48, 44, 42, 255];
const LIVE: [u8; 4] = [119, 88, 36, 255];
const ACCENT: [u8; 4] = [77, 96, 42, 255];
const BUTTON_BG: [u8; 4] = [52, 52, 60, 255];
const DPAD_BG: [u8; 4] = [96, 92, 90, 255];

const PANEL_WIDTH: usize = 960;
const PANEL_HEIGHT: usize = 656;
const MARGIN: usize = 20;
const CONTENT_WIDTH: usize = PANEL_WIDTH.saturating_sub(MARGIN.saturating_mul(2));
const HEADER_TOP: usize = 16;
const HEADER_HEIGHT: usize = 60;
const TABS_TOP: usize = 88;
const TABS_HEIGHT: usize = 36;
const ROWS_TOP: usize = 136;
const ROW_HEIGHT: usize = 52;
const ROW_STRIDE: usize = 57;
const TRIGGER_LEFT: usize = 36;
const TRIGGER_WIDTH: usize = 376;
const RESULT_LEFT: usize = 428;
const RESULT_WIDTH: usize = 376;
const HINT_LEFT: usize = 816;
const FOOTER_TOP: usize = 600;
const CORNER_RADIUS: usize = 10;

#[derive(Debug, Default)]
pub struct Buffer {
    cached: Option<Artwork>,
}

#[derive(Debug)]
struct Artwork {
    overlay: HelpOverlay,
    family: PadFamily,
    size: (i32, i32),
    scale: i32,
    buffer: MemoryRenderBuffer,
}

impl Buffer {
    /// # Errors
    /// Returns the renderer's memory import error.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        compositor: &crate::state::Compositor,
        output: &Output,
    ) -> Result<Option<MemoryRenderBufferRenderElement<R>>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(mode) = output.current_mode() else {
            return Ok(None);
        };
        let Some(overlay) = compositor.help_overlay() else {
            self.cached = None;
            return Ok(None);
        };
        let size = (mode.size.w, mode.size.h);
        let family = compositor.pad_family();
        let scale = output.current_scale().integer_scale();
        if self.cached.as_ref().is_none_or(|previous| {
            previous.overlay != *overlay
                || previous.family != family
                || previous.size != size
                || previous.scale != scale
        }) {
            self.cached = raster_scaled(overlay, family, size, scale).map(|pixels| Artwork {
                overlay: overlay.clone(),
                family,
                size,
                scale,
                buffer: MemoryRenderBuffer::from_slice(
                    &pixels,
                    Fourcc::Argb8888,
                    size,
                    1,
                    Transform::Normal,
                    None,
                ),
            });
        }
        let Some(Artwork { buffer, .. }) = &self.cached else {
            return Ok(None);
        };
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (0.0, 0.0),
            buffer,
            None,
            Some(smithay::utils::Rectangle::from_size(
                (f64::from(size.0), f64::from(size.1)).into(),
            )),
            Some(crate::scale::logical_size(mode.size, scale)),
            Kind::Unspecified,
        )
        .map(Some)
    }
}

/// Premultiplied BGRA artwork centred above the bar at scale 1.
#[must_use]
pub fn raster(overlay: &HelpOverlay, family: PadFamily, size: (i32, i32)) -> Option<Vec<u8>> {
    raster_scaled(overlay, family, size, 1)
}

/// Premultiplied BGRA artwork centred above an output's scaled bar.
#[must_use]
pub fn raster_scaled(
    overlay: &HelpOverlay,
    family: PadFamily,
    size: (i32, i32),
    scale: i32,
) -> Option<Vec<u8>> {
    let (width, height, left, top) = panel_geometry(size, scale)?;
    let mut panel = Canvas::new((width, height))?;
    panel.reference(PANEL_WIDTH, PANEL_HEIGHT);
    paint(&mut panel, overlay, family);
    let mut output = Canvas::new(size)?;
    let stride = usize::try_from(size.0).ok()?.checked_mul(4)?;
    let row_bytes = usize::try_from(width).ok()?.checked_mul(4)?;
    let left_bytes = usize::try_from(left).ok()?.checked_mul(4)?;
    let top_row = usize::try_from(top).ok()?;
    for row in 0..usize::try_from(height).ok()? {
        let source = row.checked_mul(row_bytes)?;
        let target = top_row
            .checked_add(row)?
            .checked_mul(stride)?
            .checked_add(left_bytes)?;
        output
            .pixels
            .get_mut(target..target.checked_add(row_bytes)?)?
            .copy_from_slice(panel.pixels.get(source..source.checked_add(row_bytes)?)?);
    }
    Some(output.pixels)
}

fn panel_geometry(size: (i32, i32), scale: i32) -> Option<(i32, i32, i32, i32)> {
    let scale = scale.max(1);
    let available = size
        .1
        .checked_sub(tehuti_core::bar::HEIGHT.saturating_mul(scale))?;
    let natural_width = i32::try_from(PANEL_WIDTH).ok()?;
    let natural_height = i32::try_from(PANEL_HEIGHT).ok()?;
    let max_width_from_height = available
        .saturating_mul(85)
        .checked_div(100)?
        .saturating_mul(natural_width)
        .checked_div(natural_height)?;
    let width = natural_width
        .saturating_mul(scale)
        .min(size.0.saturating_mul(9).checked_div(10)?)
        .min(max_width_from_height);
    let height = width.checked_mul(natural_height)?.checked_div(natural_width)?;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some((
        width,
        height,
        size.0.checked_sub(width)?.checked_div(2)?,
        available.checked_sub(height)?.checked_div(2)?,
    ))
}

fn paint(canvas: &mut Canvas, overlay: &HelpOverlay, family: PadFamily) {
    canvas.rectangle(0, 0, PANEL_WIDTH, PANEL_HEIGHT, PANEL_BG);
    canvas.rectangle(0, 0, PANEL_WIDTH, 1, EDGE);
    canvas.rectangle(0, PANEL_HEIGHT.saturating_sub(1), PANEL_WIDTH, 1, EDGE);
    canvas.rectangle(0, 0, 1, PANEL_HEIGHT, EDGE);
    canvas.rectangle(PANEL_WIDTH.saturating_sub(1), 0, 1, PANEL_HEIGHT, EDGE);

    paint_header(canvas, overlay, family);
    paint_tabs(canvas, overlay);
    paint_rows(canvas, overlay, family);
    paint_footer(canvas, overlay, family);
}

fn paint_header(canvas: &mut Canvas, overlay: &HelpOverlay, family: PadFamily) {
    canvas.rounded_rectangle(
        (MARGIN, HEADER_TOP, CONTENT_WIDTH, HEADER_HEIGHT),
        CORNER_RADIUS,
        HEADER_BG,
    );
    label(canvas, 36, 22, 680, "What can I say", 23, WHITE);
    let subtitle = overlay.context_name().map_or_else(
        || format!("Global  |  {}", overlay.level().label()),
        |name| format!("Focused: {name}  |  {}", overlay.level().label()),
    );
    label(canvas, 36, 50, 680, &subtitle, 15, DIM);
    paint_hint(canvas, HINT_LEFT, 32, &overlay.back_hint(family), family);
}

fn paint_tabs(canvas: &mut Canvas, overlay: &HelpOverlay) {
    let groups = overlay.groups();
    if groups.is_empty() {
        return;
    }
    let gap = 8usize;
    let total_gap = gap.saturating_mul(groups.len().saturating_sub(1));
    let tab_width = CONTENT_WIDTH
        .saturating_sub(total_gap)
        .checked_div(groups.len())
        .unwrap_or(CONTENT_WIDTH);
    for (index, group) in groups.iter().enumerate() {
        let left = MARGIN.saturating_add(index.saturating_mul(tab_width.saturating_add(gap)));
        let active = index == overlay.active_group_index();
        canvas.rounded_rectangle(
            (left, TABS_TOP, tab_width, TABS_HEIGHT),
            8,
            if active { LIVE } else { CARD_BG },
        );
        if active {
            canvas.rectangle(
                left.saturating_add(10),
                TABS_TOP.saturating_add(TABS_HEIGHT).saturating_sub(3),
                tab_width.saturating_sub(20),
                3,
                ACCENT,
            );
        }
        let title = format!("{} ({})", group.title, group.items.len());
        label(
            canvas,
            left.saturating_add(12),
            TABS_TOP.saturating_add(9),
            tab_width.saturating_sub(24),
            &title,
            15,
            if active { WHITE } else { DIM },
        );
    }
}

fn paint_rows(canvas: &mut Canvas, overlay: &HelpOverlay, family: PadFamily) {
    let Some(group) = overlay.active_group() else {
        label(
            canvas,
            36,
            ROWS_TOP.saturating_add(24),
            CONTENT_WIDTH,
            "No commands",
            20,
            DIM,
        );
        return;
    };
    let is_pad_group = matches!(group.kind, GroupKind::FocusedPad | GroupKind::GlobalPad);
    let visible = overlay.visible_range();
    for (slot, index) in visible.enumerate() {
        let Some(item) = group.items.get(index) else {
            continue;
        };
        let row_top = ROWS_TOP.saturating_add(slot.saturating_mul(ROW_STRIDE));
        let selected = index == overlay.selected_index();
        canvas.rounded_rectangle(
            (MARGIN, row_top, CONTENT_WIDTH, ROW_HEIGHT),
            8,
            if selected { LIVE } else { CARD_BG },
        );
        if selected {
            canvas.rectangle(
                MARGIN,
                row_top.saturating_add(6),
                6,
                ROW_HEIGHT.saturating_sub(12),
                ACCENT,
            );
        }
        paint_trigger(
            canvas,
            &item.trigger,
            TRIGGER_LEFT,
            row_top,
            TRIGGER_WIDTH,
            family,
        );
        paint_result(canvas, item, row_top, is_pad_group);
        if selected && let Some(confirm) = overlay.confirm_hint(family) {
            paint_hint(canvas, HINT_LEFT, row_top.saturating_add(12), &confirm, family);
        }
    }
}

fn paint_trigger(
    canvas: &mut Canvas,
    pieces: &[Piece],
    start_left: usize,
    row_top: usize,
    max_width: usize,
    family: PadFamily,
) {
    let limit = start_left.saturating_add(max_width);
    let mut cursor = start_left;
    for piece in pieces {
        if cursor >= limit {
            break;
        }
        let Some(next) = paint_trigger_piece(
            canvas,
            piece,
            cursor,
            row_top,
            limit.saturating_sub(cursor),
            family,
        ) else {
            break;
        };
        cursor = next;
    }
}

fn paint_trigger_piece(
    canvas: &mut Canvas,
    piece: &Piece,
    cursor: usize,
    row_top: usize,
    remaining: usize,
    family: PadFamily,
) -> Option<usize> {
    match piece {
        Piece::Glyph { shape, colour } => {
            if remaining < 28 {
                return None;
            }
            paint_shape(
                canvas,
                cursor,
                row_top.saturating_add(12),
                *shape,
                *colour,
                family,
            );
            Some(cursor.saturating_add(34))
        }
        Piece::Chip(text) => Some(paint_badge(canvas, cursor, row_top, remaining, text, false)),
        Piece::Key(text) => Some(paint_badge(
            canvas,
            cursor,
            row_top,
            remaining,
            &key_display(text),
            true,
        )),
        Piece::Text(text) => Some(paint_trigger_text(
            canvas,
            cursor,
            row_top.saturating_add(16),
            remaining,
            text,
            18,
            WHITE,
            10,
        )),
        Piece::Dim(text) => Some(paint_trigger_text(
            canvas,
            cursor,
            row_top.saturating_add(18),
            remaining,
            text,
            15,
            DIM,
            9,
        )),
        Piece::Icon { fallback, .. } => {
            let text = pieces_summary(fallback);
            Some(paint_trigger_text(
                canvas,
                cursor,
                row_top.saturating_add(16),
                remaining,
                &text,
                17,
                WHITE,
                10,
            ))
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "compact text-run helper keeping trigger piece rendering concise"
)]
fn paint_trigger_text(
    canvas: &mut Canvas,
    cursor: usize,
    top: usize,
    remaining: usize,
    text: &str,
    size: usize,
    colour: [u8; 4],
    char_width: usize,
) -> usize {
    label(canvas, cursor, top, remaining, text, size, colour);
    cursor.saturating_add(text.chars().count().saturating_mul(char_width).saturating_add(8))
}

fn paint_badge(
    canvas: &mut Canvas,
    cursor: usize,
    row_top: usize,
    remaining: usize,
    text: &str,
    outlined: bool,
) -> usize {
    let width = text
        .chars()
        .count()
        .saturating_mul(11)
        .saturating_add(18)
        .min(remaining);
    if outlined {
        canvas.rounded_rectangle((cursor, row_top.saturating_add(13), width, 26), 5, EDGE);
        canvas.rounded_rectangle(
            (
                cursor.saturating_add(1),
                row_top.saturating_add(14),
                width.saturating_sub(2),
                24,
            ),
            4,
            KEY_BG,
        );
    } else {
        canvas.rounded_rectangle((cursor, row_top.saturating_add(13), width, 26), 6, CHIP_BG);
    }
    label(
        canvas,
        cursor.saturating_add(8),
        row_top.saturating_add(17),
        width.saturating_sub(14),
        text,
        15,
        WHITE,
    );
    cursor.saturating_add(width).saturating_add(8)
}

fn paint_result(canvas: &mut Canvas, item: &Item, row_top: usize, is_pad_group: bool) {
    let summary = pieces_summary(&item.result);
    if is_pad_group && let Some(voice) = &item.voice_name {
        label(
            canvas,
            RESULT_LEFT,
            row_top.saturating_add(7),
            RESULT_WIDTH,
            &summary,
            18,
            WHITE,
        );
        let signpost = format!("Say \"{voice}\"");
        label(
            canvas,
            RESULT_LEFT,
            row_top.saturating_add(30),
            RESULT_WIDTH,
            &signpost,
            14,
            VOICE_HINT,
        );
    } else {
        label(
            canvas,
            RESULT_LEFT,
            row_top.saturating_add(16),
            RESULT_WIDTH,
            &summary,
            18,
            WHITE,
        );
    }
}

fn paint_footer(canvas: &mut Canvas, overlay: &HelpOverlay, family: PadFamily) {
    canvas.rectangle(MARGIN, FOOTER_TOP, CONTENT_WIDTH, 1, EDGE);
    let left_text = overlay.active_group().map_or_else(
        || "D-pad: Groups & entries".into(),
        |group| {
            if group.items.len() > VISIBLE_ROWS {
                let visible = overlay.visible_range();
                format!(
                    "D-pad: Groups & entries   ({}-{} of {})",
                    visible.start.saturating_add(1),
                    visible.end,
                    group.items.len()
                )
            } else {
                "D-pad: Groups & entries".into()
            }
        },
    );
    label(
        canvas,
        28,
        FOOTER_TOP.saturating_add(18),
        480,
        &left_text,
        15,
        DIM,
    );
    let confirm = family.label(family.button(tehuti_core::pad::Role::Confirm));
    let back = family.label(family.button(tehuti_core::pad::Role::Back));
    let right_text = format!("{confirm}: Run   {back}: Close");
    label(
        canvas,
        680,
        FOOTER_TOP.saturating_add(18),
        252,
        &right_text,
        15,
        DIM,
    );
}

fn pieces_summary(pieces: &[Piece]) -> String {
    let mut parts = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) | Piece::Dim(text) | Piece::Chip(text) => parts.push(text.clone()),
            Piece::Glyph { shape, .. } => parts.push(shape.word()),
            Piece::Key(key) => parts.push(format!("[{}]", key_display(key))),
            Piece::Icon { fallback, .. } => parts.push(pieces_summary(fallback)),
        }
    }
    parts.join(" ")
}

fn key_display(label: &str) -> String {
    match label {
        "\u{23ce}" => "Enter".into(),
        "\u{232b}" => "Bksp".into(),
        "\u{2191}" => "Up".into(),
        "\u{2193}" => "Down".into(),
        "\u{2190}" => "Left".into(),
        "\u{2192}" => "Right".into(),
        other => other.into(),
    }
}

fn label(
    canvas: &mut Canvas,
    left: usize,
    top: usize,
    width: usize,
    text: &str,
    size: usize,
    colour: [u8; 4],
) {
    canvas.ellipsized_line(
        (left, top, width, size.saturating_add(8)),
        &[(text, colour)],
        size,
    );
}

fn paint_hint(canvas: &mut Canvas, left: usize, top: usize, hint: &(Piece, &str), family: PadFamily) {
    if let Piece::Glyph { shape, colour } = hint.0 {
        paint_shape(canvas, left, top, shape, colour, family);
    }
    label(
        canvas,
        left.saturating_add(34),
        top.saturating_add(5),
        86,
        hint.1,
        16,
        WHITE,
    );
}

fn paint_shape(canvas: &mut Canvas, left: usize, top: usize, shape: Shape, colour: Rgb, family: PadFamily) {
    let Rgb(red, green, blue) = colour;
    let bgra = [blue, green, red, 255];
    match shape {
        Shape::Disc(letter)
            if matches!(
                family,
                PadFamily::Nintendo | PadFamily::Remote | PadFamily::RemotePaired
            ) =>
        {
            disc(canvas, left, top, BUTTON_BG, false);
            canvas.centred(disc_centre(left, top), &letter.to_string(), 19, WHITE);
        }
        Shape::Disc(letter) => {
            disc(canvas, left, top, bgra, false);
            canvas.centred(disc_centre(left, top), &letter.to_string(), 19, [20, 20, 20, 255]);
        }
        Shape::Dpad(direction) => paint_dpad(canvas, left, top, direction, bgra),
        Shape::Ring | Shape::Cross | Shape::Square | Shape::Triangle | Shape::View | Shape::Menu => {
            paint_button_symbol(canvas, left, top, shape, bgra);
        }
    }
}

fn paint_dpad(canvas: &mut Canvas, left: usize, top: usize, direction: Direction, bgra: [u8; 4]) {
    canvas.rectangle(left.saturating_add(10), top.saturating_add(2), 8, 24, DPAD_BG);
    canvas.rectangle(left.saturating_add(2), top.saturating_add(10), 24, 8, DPAD_BG);
    match direction {
        Direction::Upward => canvas.rectangle(left.saturating_add(10), top.saturating_add(2), 8, 10, bgra),
        Direction::Down => canvas.rectangle(left.saturating_add(10), top.saturating_add(16), 8, 10, bgra),
        Direction::Left => canvas.rectangle(left.saturating_add(2), top.saturating_add(10), 10, 8, bgra),
        Direction::Right => canvas.rectangle(left.saturating_add(16), top.saturating_add(10), 10, 8, bgra),
    }
}

fn paint_button_symbol(canvas: &mut Canvas, left: usize, top: usize, shape: Shape, bgra: [u8; 4]) {
    disc(canvas, left, top, BUTTON_BG, false);
    match shape {
        Shape::Ring => disc(canvas, left, top, bgra, true),
        Shape::Cross => {
            for offset in 0usize..16 {
                canvas.rectangle(
                    left.saturating_add(5).saturating_add(offset),
                    top.saturating_add(5).saturating_add(offset),
                    2,
                    2,
                    bgra,
                );
                canvas.rectangle(
                    left.saturating_add(20).saturating_sub(offset),
                    top.saturating_add(5).saturating_add(offset),
                    2,
                    2,
                    bgra,
                );
            }
        }
        Shape::Square => {
            canvas.rectangle(left.saturating_add(7), top.saturating_add(7), 14, 2, bgra);
            canvas.rectangle(left.saturating_add(7), top.saturating_add(19), 14, 2, bgra);
            canvas.rectangle(left.saturating_add(7), top.saturating_add(7), 2, 14, bgra);
            canvas.rectangle(left.saturating_add(19), top.saturating_add(7), 2, 14, bgra);
        }
        Shape::Triangle => paint_triangle(canvas, left, top, bgra),
        Shape::View => {
            canvas.rectangle(left.saturating_add(6), top.saturating_add(7), 10, 8, bgra);
            canvas.rectangle(left.saturating_add(12), top.saturating_add(13), 10, 8, bgra);
        }
        Shape::Menu => {
            for bar_top in [8usize, 13, 18] {
                canvas.rectangle(left.saturating_add(7), top.saturating_add(bar_top), 14, 2, bgra);
            }
        }
        Shape::Disc(_) | Shape::Dpad(_) => {}
    }
}

fn paint_triangle(canvas: &mut Canvas, left: usize, top: usize, bgra: [u8; 4]) {
    canvas.rectangle(left.saturating_add(7), top.saturating_add(19), 14, 2, bgra);
    for step in 0usize..12 {
        let half = step.checked_div(2).unwrap_or_default();
        canvas.rectangle(
            left.saturating_add(13).saturating_sub(half),
            top.saturating_add(7).saturating_add(step),
            2,
            2,
            bgra,
        );
        canvas.rectangle(
            left.saturating_add(13).saturating_add(half),
            top.saturating_add(7).saturating_add(step),
            2,
            2,
            bgra,
        );
    }
}

const fn disc_centre(left: usize, top: usize) -> (usize, usize) {
    (
        left.saturating_mul(2).saturating_add(28),
        top.saturating_mul(2).saturating_add(28),
    )
}

fn disc(canvas: &mut Canvas, left: usize, top: usize, colour: [u8; 4], ring: bool) {
    for row in 0_i32..28 {
        for column in 0_i32..28 {
            let horizontal = column.saturating_sub(14);
            let vertical = row.saturating_sub(14);
            let distance = horizontal
                .saturating_mul(horizontal)
                .saturating_add(vertical.saturating_mul(vertical));
            if distance <= 169 && (!ring || distance >= 100) {
                canvas.rectangle(
                    left.saturating_add(usize::try_from(column).unwrap_or_default()),
                    top.saturating_add(usize::try_from(row).unwrap_or_default()),
                    1,
                    1,
                    colour,
                );
            }
        }
    }
}
