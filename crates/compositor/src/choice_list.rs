//! Native-resolution ranked choice artwork and its cached overlay element.

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
    choice_list::{Choice, List},
    pad::PadFamily,
    ticker::{Piece, Rgb, Shape},
};

const WHITE: [u8; 4] = [240, 236, 229, 255];
const DIM: [u8; 4] = [151, 143, 132, 255];
const EDGE: [u8; 4] = [98, 86, 72, 255];
const GREEN: [u8; 4] = [88, 107, 61, 255];
const LIVE: [u8; 4] = [119, 88, 36, 255];
const RESULT: [u8; 4] = [77, 96, 42, 255];
const LEFT: usize = 1280;
const WIDTH: usize = 640;
const INSET: usize = 24;

#[derive(Debug, Default)]
pub struct Buffer {
    cached: Option<(List, PadFamily, (i32, i32), MemoryRenderBuffer)>,
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
        let Some(list) = compositor.choice_list().filter(|list| list.is_open()) else {
            self.cached = None;
            return Ok(None);
        };
        let size = (mode.size.w, mode.size.h);
        let family = compositor.pad_family();
        if self.cached.as_ref().is_none_or(|(previous, pad, dimensions, _)| {
            previous != list || *pad != family || *dimensions != size
        }) {
            self.cached = raster(list, family, size).map(|pixels| {
                (
                    list.clone(),
                    family,
                    size,
                    MemoryRenderBuffer::from_slice(
                        &pixels,
                        Fourcc::Argb8888,
                        size,
                        1,
                        Transform::Normal,
                        None,
                    ),
                )
            });
        }
        let Some((_, _, _, buffer)) = &self.cached else {
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
            Some(crate::scale::logical_size(
                mode.size,
                output.current_scale().integer_scale(),
            )),
            Kind::Unspecified,
        )
        .map(Some)
    }
}

/// Premultiplied BGRA pixels, transparent outside the right-hand column.
#[must_use]
pub fn raster(list: &List, family: PadFamily, size: (i32, i32)) -> Option<Vec<u8>> {
    if !list.is_open() {
        return None;
    }
    let mut canvas = Canvas::new(size)?;
    canvas.reference(1920, 1080);
    paint(&mut canvas, list, family);
    Some(canvas.pixels)
}

fn label(canvas: &mut Canvas, x: usize, y: usize, width: usize, text: &str, size: usize, colour: [u8; 4]) {
    canvas.line((x, y, width, size.saturating_add(6)), &[(text, colour)], size);
}

fn border(canvas: &mut Canvas, y: usize, height: usize, colour: [u8; 4], dashed: bool) {
    let x = LEFT.saturating_add(12);
    let width = WIDTH.saturating_sub(24);
    let step = if dashed { 16 } else { width };
    let length = if dashed { 8 } else { width };
    for offset in (0..width).step_by(step) {
        canvas.rectangle(
            x.saturating_add(offset),
            y,
            length.min(width.saturating_sub(offset)),
            1,
            colour,
        );
        canvas.rectangle(
            x.saturating_add(offset),
            y.saturating_add(height).saturating_sub(1),
            length.min(width.saturating_sub(offset)),
            1,
            colour,
        );
    }
    for offset in (0..height).step_by(if dashed { 16 } else { height }) {
        let side_length = if dashed {
            8.min(height.saturating_sub(offset))
        } else {
            height
        };
        canvas.rectangle(x, y.saturating_add(offset), 1, side_length, colour);
        canvas.rectangle(
            x.saturating_add(width).saturating_sub(1),
            y.saturating_add(offset),
            1,
            side_length,
            colour,
        );
    }
}

fn paint(canvas: &mut Canvas, list: &List, family: PadFamily) {
    let scopes = list.slots().scopes.len();
    let header = 84usize.saturating_add(scopes.saturating_mul(24));
    let create_height: usize = if list.slots().create.is_some() { 128 } else { 0 };
    let rows = list
        .visible()
        .len()
        .max(usize::from(list.slots().create.is_none()));
    let height = header
        .saturating_add(create_height)
        .saturating_add(rows.saturating_mul(72))
        .saturating_add(68);
    panel(canvas, height);
    paint_header(canvas, list, family, header);
    let mut y = 48usize.saturating_add(header);
    paint_create(canvas, list, family, y);
    y = y.saturating_add(create_height);
    let mut section = None;
    for row in list.candidates().get(list.visible()).unwrap_or_default() {
        if list.show_headings() && section != Some(row.close) {
            let heading = if row.close {
                &list.slots().closest_heading
            } else {
                &list.slots().remaining_heading
            };
            label(canvas, LEFT.saturating_add(INSET), y, 592, heading, 14, DIM);
            y = y.saturating_add(28);
            section = Some(row.close);
        }
        paint_candidate(canvas, list, family, row, y);
        y = y.saturating_add(72);
    }
    label(
        canvas,
        LEFT.saturating_add(INSET),
        y.saturating_add(36),
        592,
        list.footer(),
        16,
        DIM,
    );
    if list.candidates().is_empty() && list.slots().create.is_none() {
        label(
            canvas,
            LEFT.saturating_add(INSET),
            y.saturating_add(18),
            592,
            "Nothing found",
            25,
            DIM,
        );
    }
}

fn panel(canvas: &mut Canvas, height: usize) {
    for spread in (1usize..=12).rev() {
        canvas.rectangle(
            LEFT.saturating_sub(spread),
            36usize.saturating_sub(spread),
            WIDTH.saturating_add(spread),
            height.saturating_add(spread.saturating_mul(2)),
            [0, 0, 0, 4],
        );
    }
    canvas.rectangle(LEFT, 36, WIDTH, height, [30, 24, 20, 240]);
    canvas.rectangle(LEFT, 36, 1, height, EDGE);
    canvas.rectangle(LEFT.saturating_add(WIDTH).saturating_sub(1), 36, 1, height, EDGE);
    canvas.rectangle(LEFT, 36, WIDTH, 1, EDGE);
    canvas.rectangle(LEFT, 36usize.saturating_add(height), WIDTH, 1, EDGE);
}

fn paint_header(canvas: &mut Canvas, list: &List, family: PadFamily, height: usize) {
    canvas.rectangle(
        LEFT.saturating_add(1),
        37,
        WIDTH.saturating_sub(2),
        height,
        [88, 65, 47, 255],
    );
    label(
        canvas,
        LEFT.saturating_add(INSET),
        54,
        430,
        &list.slots().title,
        34,
        WHITE,
    );
    hint(
        canvas,
        LEFT.saturating_add(508),
        56,
        &list.back_hint(family),
        family,
    );
    for (index, scope) in list.slots().scopes.iter().enumerate() {
        label(
            canvas,
            LEFT.saturating_add(INSET),
            100usize.saturating_add(index.saturating_mul(24)),
            WIDTH.saturating_sub(48),
            scope,
            17,
            DIM,
        );
    }
}

fn paint_create(canvas: &mut Canvas, list: &List, family: PadFamily, y: usize) {
    let Some(create) = &list.slots().create else {
        return;
    };
    let selected = list.selected() == Some(Choice::Create);
    if selected {
        canvas.rectangle(
            LEFT.saturating_add(12),
            y,
            WIDTH.saturating_sub(24),
            112,
            [48, 58, 29, 255],
        );
        canvas.rectangle(LEFT.saturating_add(12), y, 8, 112, RESULT);
    }
    border(canvas, y, 112, if selected { RESULT } else { GREEN }, !selected);
    label(
        canvas,
        LEFT.saturating_add(INSET),
        y.saturating_add(12),
        440,
        &create.heading,
        14,
        GREEN,
    );
    label(
        canvas,
        LEFT.saturating_add(INSET),
        y.saturating_add(38),
        if selected { 448 } else { 592 },
        &list.slots().heard,
        26,
        WHITE,
    );
    label(
        canvas,
        LEFT.saturating_add(INSET),
        y.saturating_add(80),
        592,
        &create.detail,
        16,
        DIM,
    );
    selected_hint(canvas, list, family, y.saturating_add(38), selected);
}

fn paint_candidate(
    canvas: &mut Canvas,
    list: &List,
    family: PadFamily,
    row: &tehuti_core::choice_list::Ranked,
    y: usize,
) {
    let selected = list.selected() == Some(row.choice);
    if selected {
        canvas.rectangle(LEFT.saturating_add(12), y, WIDTH.saturating_sub(24), 72, LIVE);
        canvas.rectangle(LEFT.saturating_add(12), y, 8, 72, [230, 215, 178, 255]);
    }
    if list
        .candidates()
        .first()
        .is_some_and(|first| first.choice == row.choice)
        && list.fill() > 0
    {
        canvas.rectangle(
            LEFT.saturating_add(12),
            y,
            WIDTH
                .saturating_sub(24)
                .saturating_mul(usize::from(list.fill()))
                .checked_div(1000)
                .unwrap_or_default(),
            72,
            RESULT,
        );
    }
    let (bright, tail) = row.name_spans();
    canvas.line(
        (
            LEFT.saturating_add(INSET),
            y.saturating_add(9),
            if selected { 448 } else { 592 },
            36,
        ),
        &[(bright, WHITE), (tail, DIM)],
        27,
    );
    if let Some(meta) = &row.candidate.meta {
        label(
            canvas,
            LEFT.saturating_add(INSET),
            y.saturating_add(45),
            592,
            meta,
            16,
            DIM,
        );
    }
    selected_hint(canvas, list, family, y.saturating_add(12), selected);
}

fn selected_hint(canvas: &mut Canvas, list: &List, family: PadFamily, y: usize, selected: bool) {
    if selected && let Some(selected_hint) = list.selected_hint(family) {
        hint(canvas, LEFT.saturating_add(508), y, &selected_hint, family);
    }
}

fn hint(canvas: &mut Canvas, x: usize, y: usize, hint: &(Piece, &str), family: PadFamily) {
    if let Piece::Glyph {
        shape,
        colour: Rgb(red, green, blue),
    } = hint.0
    {
        let colour = [blue, green, red, 255];
        match shape {
            Shape::Disc(letter) if family == PadFamily::Nintendo => {
                label(canvas, x.saturating_add(5), y, 24, &letter.to_string(), 23, WHITE);
            }
            Shape::Disc(letter) => {
                disc(canvas, x, y, colour, false);
                label(
                    canvas,
                    x.saturating_add(6),
                    y.saturating_add(1),
                    22,
                    &letter.to_string(),
                    20,
                    [20, 20, 20, 255],
                );
            }
            Shape::Ring => disc(canvas, x, y, colour, true),
            Shape::Cross => {
                for offset in 0usize..18 {
                    canvas.rectangle(
                        x.saturating_add(4).saturating_add(offset),
                        y.saturating_add(4).saturating_add(offset),
                        3,
                        3,
                        colour,
                    );
                    canvas.rectangle(
                        x.saturating_add(21).saturating_sub(offset),
                        y.saturating_add(4).saturating_add(offset),
                        3,
                        3,
                        colour,
                    );
                }
            }
            Shape::Square | Shape::Triangle | Shape::Dpad(_) | Shape::View | Shape::Menu => {}
        }
    }
    label(
        canvas,
        x.saturating_add(34),
        y.saturating_add(4),
        94,
        hint.1,
        17,
        WHITE,
    );
}

fn disc(canvas: &mut Canvas, x: usize, y: usize, colour: [u8; 4], ring: bool) {
    for row in 0_i32..28 {
        for column in 0_i32..28 {
            let horizontal = column.saturating_sub(14);
            let vertical = row.saturating_sub(14);
            let distance = horizontal
                .saturating_mul(horizontal)
                .saturating_add(vertical.saturating_mul(vertical));
            if distance <= 169 && (!ring || distance >= 100) {
                canvas.rectangle(
                    x.saturating_add(usize::try_from(column).unwrap_or_default()),
                    y.saturating_add(usize::try_from(row).unwrap_or_default()),
                    1,
                    1,
                    colour,
                );
            }
        }
    }
}
