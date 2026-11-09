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
const INSET: usize = 32;
const CONTENT_WIDTH: usize = WIDTH.saturating_sub(INSET.saturating_mul(2));
const ROW_TEXT_LEFT: usize = INSET.saturating_add(56);
const ROW_TEXT_WIDTH: usize = WIDTH.saturating_sub(ROW_TEXT_LEFT).saturating_sub(INSET);
const CORNER_RADIUS: usize = 12;
const NAME_SIZE: usize = 26;
const DETAIL_SIZE: usize = 16;
const HINT_LEFT: usize = 508;
const HINT_GAP: usize = 16;
const SELECTED_TEXT_WIDTH: usize = HINT_LEFT.saturating_sub(ROW_TEXT_LEFT).saturating_sub(HINT_GAP);

#[derive(Debug, Default)]
pub struct Buffer {
    cached: Option<Artwork>,
    icon_names: Vec<String>,
    icons: Vec<Option<image::DynamicImage>>,
    pending: Option<std::sync::mpsc::Receiver<(usize, Option<image::DynamicImage>)>>,
}

#[derive(Debug)]
struct Artwork {
    list: List,
    family: PadFamily,
    size: (i32, i32),
    scale: i32,
    buffer: MemoryRenderBuffer,
}

impl Buffer {
    fn update_icons(&mut self, icon_names: Vec<String>) {
        if self.icon_names != icon_names {
            let paths = icon_names
                .iter()
                .map(|name| crate::desktop_hints::resolve_icon(name))
                .collect();
            self.icons = icon_names.iter().map(|_| None).collect();
            self.pending = Some(crate::desktop_hints::spawn_icon_loader(paths));
            self.icon_names = icon_names;
            self.cached = None;
        }
        if let Some(receiver) = &self.pending {
            while let Ok((index, icon)) = receiver.try_recv() {
                if let Some(slot) = self.icons.get_mut(index) {
                    *slot = icon;
                    self.cached = None;
                }
            }
        }
    }

    /// Waits (bounded) for pending icons; only offline shots call this, never a live frame.
    pub(crate) fn settle_icons(&mut self, icon_names: Vec<String>, budget: core::time::Duration) {
        self.update_icons(icon_names);
        let Some(deadline) = std::time::Instant::now().checked_add(budget) else {
            return;
        };
        let mut remaining = self.icons.iter().filter(|icon| icon.is_none()).count();
        while remaining > 0 {
            let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else {
                break;
            };
            let Some(receiver) = &self.pending else { break };
            let Ok((index, icon)) = receiver.recv_timeout(left) else {
                break;
            };
            if let Some(slot) = self.icons.get_mut(index) {
                *slot = icon;
                self.cached = None;
            }
            remaining = remaining.saturating_sub(1);
        }
    }

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
        self.update_icons(compositor.launcher_icons());
        let size = (mode.size.w, mode.size.h);
        let family = compositor.pad_family();
        let scale = output.current_scale().integer_scale();
        if self.cached.as_ref().is_none_or(|previous| {
            previous.list != *list
                || previous.family != family
                || previous.size != size
                || previous.scale != scale
        }) {
            self.cached = raster_with_icons(list, family, size, scale, &self.icons).map(|pixels| Artwork {
                list: list.clone(),
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
            Some(crate::scale::logical_size(
                mode.size,
                output.current_scale().integer_scale(),
            )),
            Kind::Unspecified,
        )
        .map(Some)
    }
}

/// Premultiplied BGRA artwork, positioned by the overlay element.
#[must_use]
pub fn raster(list: &List, family: PadFamily, size: (i32, i32)) -> Option<Vec<u8>> {
    raster_scaled(list, family, size, 1)
}

/// Like [`raster`], for an output whose integer `scale` also scales the bar.
#[must_use]
pub fn raster_scaled(list: &List, family: PadFamily, size: (i32, i32), scale: i32) -> Option<Vec<u8>> {
    raster_with_icons(list, family, size, scale, &[])
}

fn raster_with_icons(
    list: &List,
    family: PadFamily,
    size: (i32, i32),
    scale: i32,
    icons: &[Option<image::DynamicImage>],
) -> Option<Vec<u8>> {
    if !list.is_open() {
        return None;
    }
    let (width, height, left, top) = panel_geometry(list, size, scale)?;
    let natural = content_height(list);
    let mut canvas = Canvas::new((width.checked_mul(3)?, height))?;
    canvas.reference(1920, natural);
    let _layout = paint(&mut canvas, list, family, natural, icons);
    let mut output = Canvas::new(size)?;
    let stride = usize::try_from(size.0).ok()?.checked_mul(4)?;
    let row_bytes = usize::try_from(width).ok()?.checked_mul(4)?;
    for row in 0..usize::try_from(height).ok()? {
        let source = row
            .checked_mul(row_bytes.checked_mul(3)?)?
            .checked_add(row_bytes.checked_mul(2)?)?;
        let target = usize::try_from(top)
            .ok()?
            .checked_add(row)?
            .checked_mul(stride)?
            .checked_add(usize::try_from(left).ok()?.checked_mul(4)?)?;
        output
            .pixels
            .get_mut(target..target.checked_add(row_bytes)?)?
            .copy_from_slice(canvas.pixels.get(source..source.checked_add(row_bytes)?)?);
    }
    Some(output.pixels)
}

fn content_height(list: &List) -> usize {
    let mut height = 132usize.saturating_add(list.slots().scopes.len().saturating_mul(24));
    if list.slots().create.is_some() {
        height = height.saturating_add(128);
    }
    let mut section = None;
    for row in list.candidates().get(list.visible()).unwrap_or_default() {
        if list.show_headings() && section != Some(row.close) {
            height = height.saturating_add(28);
            section = Some(row.close);
        }
        height = height.saturating_add(72);
    }
    height.saturating_add(FOOTER)
}

fn panel_geometry(list: &List, size: (i32, i32), scale: i32) -> Option<(i32, i32, i32, i32)> {
    let scale = scale.max(1);
    let available = size
        .1
        .checked_sub(tehuti_core::bar::HEIGHT.saturating_mul(scale))?;
    let natural = i32::try_from(content_height(list)).ok()?;
    let width = 640i32
        .saturating_mul(scale)
        .min(size.0.saturating_mul(9).checked_div(10)?)
        .min(
            available
                .saturating_mul(4)
                .checked_div(5)?
                .saturating_mul(640)
                .checked_div(natural)?,
        );
    let height = width.checked_mul(natural)?.checked_div(640)?;
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

fn label(canvas: &mut Canvas, x: usize, y: usize, width: usize, text: &str, size: usize, colour: [u8; 4]) {
    canvas.ellipsized_line((x, y, width, size.saturating_add(6)), &[(text, colour)], size);
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

const PANEL_TOP: usize = 0;
const FOOTER: usize = 56;

fn paint(
    canvas: &mut Canvas,
    list: &List,
    family: PadFamily,
    bottom: usize,
    icons: &[Option<image::DynamicImage>],
) -> serde_json::Value {
    let mut row_bounds = Vec::new();
    let scopes = list.slots().scopes.len();
    let header = 84usize.saturating_add(scopes.saturating_mul(24));
    let create_height: usize = if list.slots().create.is_some() { 128 } else { 0 };
    let height = bottom.saturating_sub(PANEL_TOP);
    panel(canvas, height);
    paint_header(canvas, list, family);
    let mut y = 48usize.saturating_add(header);
    paint_create(canvas, list, family, y);
    y = y.saturating_add(create_height);
    let rows = list.candidates().get(list.visible()).unwrap_or_default();
    let footer_top = bottom.saturating_sub(FOOTER);
    let row_height = 72;
    let mut section = None;
    for row in rows {
        if list.show_headings() && section != Some(row.close) {
            let heading = if row.close {
                &list.slots().closest_heading
            } else {
                &list.slots().remaining_heading
            };
            label(
                canvas,
                LEFT.saturating_add(INSET),
                y,
                CONTENT_WIDTH,
                heading,
                14,
                DIM,
            );
            y = y.saturating_add(28);
            section = Some(row.close);
        }
        row_bounds.push(serde_json::json!({"y": y, "height": row_height}));
        let icon = match row.choice {
            Choice::Candidate(index) => icons.get(index).and_then(Option::as_ref),
            Choice::Create => None,
        };
        paint_candidate(canvas, list, family, row, (y, row_height), icon);
        y = y.saturating_add(row_height);
    }
    paint_footer(canvas, list, y, footer_top);
    serde_json::json!({"reference": [1920, 1080], "panel": {"y": PANEL_TOP, "height": height}, "rows": row_bounds, "rows_bottom": y, "glyphs": canvas.glyphs})
}

fn paint_footer(canvas: &mut Canvas, list: &List, y: usize, footer_top: usize) {
    if list.candidates().is_empty() && list.slots().create.is_none() {
        let message = if list.footer().is_empty() {
            "Nothing found"
        } else {
            list.footer()
        };
        label(
            canvas,
            LEFT.saturating_add(INSET),
            y.saturating_add(18),
            CONTENT_WIDTH,
            message,
            25,
            DIM,
        );
    } else {
        label(
            canvas,
            LEFT.saturating_add(INSET),
            footer_top.saturating_add(18),
            CONTENT_WIDTH,
            list.footer(),
            DETAIL_SIZE,
            DIM,
        );
    }
}

/// Layout reported by the same paint pass used for screenshots.
#[must_use]
pub fn scene(list: &List, family: PadFamily, size: (i32, i32)) -> Option<serde_json::Value> {
    let (width, height, left, top) = panel_geometry(list, size, 1)?;
    let mut canvas = Canvas::new(size)?;
    canvas.reference(1920, 1080);
    canvas.trace = true;
    let mut layout = paint(&mut canvas, list, family, content_height(list), &[]);
    *layout.get_mut("panel")? = serde_json::json!({"x": left, "y": top, "width": width, "height": height});
    Some(layout)
}

fn panel(canvas: &mut Canvas, height: usize) {
    for spread in (1usize..=12).rev() {
        canvas.rectangle(
            LEFT.saturating_sub(spread),
            PANEL_TOP.saturating_sub(spread),
            WIDTH.saturating_add(spread),
            height.saturating_add(spread.saturating_mul(2)),
            [0, 0, 0, 4],
        );
    }
    canvas.rectangle(LEFT, PANEL_TOP, WIDTH, height, [30, 24, 20, 240]);
    canvas.rectangle(LEFT, PANEL_TOP, 1, height, EDGE);
    canvas.rectangle(
        LEFT.saturating_add(WIDTH).saturating_sub(1),
        PANEL_TOP,
        1,
        height,
        EDGE,
    );
    canvas.rectangle(LEFT, PANEL_TOP, WIDTH, 1, EDGE);
    canvas.rectangle(
        LEFT,
        PANEL_TOP.saturating_add(height).saturating_sub(1),
        WIDTH,
        1,
        EDGE,
    );
}

fn paint_header(canvas: &mut Canvas, list: &List, family: PadFamily) {
    canvas.rounded_rectangle(
        (LEFT.saturating_add(24), 36, WIDTH.saturating_sub(48), 56),
        CORNER_RADIUS,
        [53, 44, 37, 255],
    );
    label(
        canvas,
        LEFT.saturating_add(INSET),
        50,
        440,
        &list.slots().title,
        NAME_SIZE,
        WHITE,
    );
    hint(
        canvas,
        LEFT.saturating_add(HINT_LEFT),
        50,
        &list.back_hint(family),
        family,
    );
    for (index, scope) in list.slots().scopes.iter().enumerate() {
        label(
            canvas,
            LEFT.saturating_add(INSET),
            100usize.saturating_add(index.saturating_mul(24)),
            CONTENT_WIDTH,
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
        if selected {
            SELECTED_TEXT_WIDTH
        } else {
            CONTENT_WIDTH
        },
        &list.slots().heard,
        26,
        WHITE,
    );
    label(
        canvas,
        LEFT.saturating_add(INSET),
        y.saturating_add(80),
        CONTENT_WIDTH,
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
    bounds: (usize, usize),
    icon: Option<&image::DynamicImage>,
) {
    let (top, height) = bounds;
    let y = top.saturating_add(height.saturating_sub(72) / 2);
    let selected = list.selected() == Some(row.choice);
    canvas.rounded_rectangle(
        (LEFT.saturating_add(12), top, WIDTH.saturating_sub(24), height),
        CORNER_RADIUS,
        if selected { LIVE } else { [43, 36, 30, 255] },
    );
    paint_leading(canvas, &row.candidate.name, y, (selected, icon));
    if list
        .candidates()
        .first()
        .is_some_and(|first| first.choice == row.choice)
        && list.fill() > 0
    {
        canvas.rectangle(
            LEFT.saturating_add(12),
            top.saturating_add(height).saturating_sub(3),
            WIDTH
                .saturating_sub(24)
                .saturating_mul(usize::from(list.fill()))
                .checked_div(1000)
                .unwrap_or_default(),
            3,
            RESULT,
        );
    }
    let (bright, tail) = row.name_spans();
    canvas.ellipsized_line(
        (
            LEFT.saturating_add(ROW_TEXT_LEFT),
            y.saturating_add(9),
            if selected {
                SELECTED_TEXT_WIDTH
            } else {
                ROW_TEXT_WIDTH
            },
            36,
        ),
        &[(bright, WHITE), (tail, DIM)],
        NAME_SIZE,
    );
    if let Some(meta) = &row.candidate.meta {
        label(
            canvas,
            LEFT.saturating_add(ROW_TEXT_LEFT),
            y.saturating_add(45),
            ROW_TEXT_WIDTH,
            meta,
            DETAIL_SIZE,
            DIM,
        );
    }
    selected_hint(canvas, list, family, y.saturating_add(12), selected);
}

fn paint_leading(
    canvas: &mut Canvas,
    name: &str,
    y: usize,
    (selected, icon): (bool, Option<&image::DynamicImage>),
) {
    if let Some(icon) = icon {
        canvas.bgra_image_box(LEFT.saturating_add(INSET), y.saturating_add(16), 40, icon);
    } else {
        paint_initial(canvas, name, y, selected);
    }
}

fn paint_initial(canvas: &mut Canvas, name: &str, y: usize, selected: bool) {
    canvas.rounded_rectangle(
        (LEFT.saturating_add(INSET), y.saturating_add(16), 40, 40),
        10,
        if selected {
            [151, 117, 57, 255]
        } else {
            [68, 57, 46, 255]
        },
    );
    let initial = name
        .chars()
        .find(|character| !character.is_whitespace())
        .unwrap_or('·')
        .to_string();
    canvas.centred(
        (
            LEFT.saturating_add(INSET).saturating_add(20).saturating_mul(2),
            y.saturating_add(36).saturating_mul(2),
        ),
        &initial,
        22,
        WHITE,
    );
}

fn selected_hint(canvas: &mut Canvas, list: &List, family: PadFamily, y: usize, selected: bool) {
    if selected && let Some(selected_hint) = list.selected_hint(family) {
        hint(canvas, LEFT.saturating_add(HINT_LEFT), y, &selected_hint, family);
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
                canvas.centred(disc_centre(x, y), &letter.to_string(), 23, WHITE);
            }
            Shape::Disc(letter) => {
                disc(canvas, x, y, colour, false);
                let before = canvas.trace.then(|| canvas.pixels.clone());
                canvas.centred(disc_centre(x, y), &letter.to_string(), 20, [20, 20, 20, 255]);
                if let Some(before) = before {
                    canvas.record_glyph(&before, (x, y, 28));
                }
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

/// The middle of the 28-unit disc drawn at (`x`, `y`), doubled: its pixels span 1..=27.
const fn disc_centre(x: usize, y: usize) -> (usize, usize) {
    (
        x.saturating_mul(2).saturating_add(29),
        y.saturating_mul(2).saturating_add(29),
    )
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
