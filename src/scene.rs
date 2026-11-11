//! What there is to draw for one frame.
//!
//! The scene is filled while the session is locked (reading the grids), then
//! drawn once the lock is released: text shaping, which is slower, thus
//! never blocks reading from the shell.

use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{CursorShape, NamedColor, Rgb};
use gpui::{
    App, Bounds, ContentMask, Font, FontStyle, FontWeight, Hsla, PaintQuad, Pixels, Point,
    StrikethroughStyle, TextRun, UnderlineStyle, Window, fill, outline, point, px, size,
};

use crate::palette::{Palette, dim, hsla};

/// Font and dimensions of a cell.
#[derive(Clone)]
pub struct Metrics {
    pub font: Font,
    pub font_size: Pixels,
    pub cell_width: Pixels,
    pub cell_height: Pixels,
}

/// UI text (block header, editor), outside the terminal grid.
pub struct Label {
    pub origin: Point<Pixels>,
    pub text: String,
    pub color: Hsla,
    pub bold: bool,
    pub underline: bool,
    /// Snaps each character to a cell (text without wide characters).
    pub on_grid: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    foreground: Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

/// A run of neighboring cells with the same style, drawn in a single call.
struct TextBatch {
    y: Pixels,
    column: usize,
    cells: usize,
    text: String,
    style: CellStyle,
    /// A wide or combining character: it gets a batch of its own, because
    /// glyph placement assumes one glyph per cell.
    isolated: bool,
}

struct BackgroundRun {
    y: Pixels,
    column: usize,
    cells: usize,
    color: Rgb,
}

struct CursorPaint {
    y: Pixels,
    column: usize,
    cells: usize,
    shape: CursorShape,
}

/// The colors to interpret a grid's cells with.
pub struct RowColors<'a> {
    pub palette: &'a Palette,
    /// Colors redefined by the running program (OSC 4, 10, 11…).
    pub overrides: &'a Colors,
}

/// A layer: backgrounds, terminal cells, UI text, then strokes.
#[derive(Default)]
pub struct Layer {
    pub quads_below: Vec<PaintQuad>,
    backgrounds: Vec<BackgroundRun>,
    batches: Vec<TextBatch>,
    cursor: Option<CursorPaint>,
    pub labels: Vec<Label>,
    pub quads_above: Vec<PaintQuad>,
}

impl Layer {
    /// Adds a row of cells at height `y`.
    ///
    /// `cursor` gives the cursor's column and shape if it is on this row.
    pub fn push_row<'a>(
        &mut self,
        y: Pixels,
        cells: impl Iterator<Item = &'a Cell>,
        colors: &RowColors<'_>,
        selected: impl Fn(usize) -> bool,
        cursor: Option<(usize, CursorShape)>,
    ) {
        let palette = colors.palette;
        let default_background = palette.indexed(NamedColor::Background as usize, colors.overrides);
        let first_background = self.backgrounds.len();
        let first_batch = self.batches.len();
        let mut cursor_on_cell = false;

        for (column, cell) in cells.enumerate() {
            let flags = cell.flags;
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let cells = if flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };

            let mut foreground = palette.resolve(cell.fg, colors.overrides);
            let mut background = palette.resolve(cell.bg, colors.overrides);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut foreground, &mut background);
            }
            if flags.contains(Flags::DIM) {
                foreground = dim(foreground, background);
            }
            if selected(column) {
                background = palette.selection;
            }
            if flags.contains(Flags::HIDDEN) {
                foreground = background;
            }

            if let Some((cursor_column, shape)) = cursor
                && cursor_column == column
            {
                cursor_on_cell = true;
                self.cursor = Some(CursorPaint { y, column, cells, shape });
                if shape == CursorShape::Block {
                    foreground = default_background;
                }
            }

            if background != default_background {
                match self.backgrounds[first_background..].last_mut() {
                    Some(run) if run.column + run.cells == column && run.color == background => {
                        run.cells += cells;
                    }
                    _ => self.backgrounds.push(BackgroundRun { y, column, cells, color: background }),
                }
            }

            let style = CellStyle {
                foreground,
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                strikethrough: flags.contains(Flags::STRIKEOUT),
            };
            let zero_width = cell.zerowidth();
            let isolated = cells == 2 || zero_width.is_some();
            let blank = matches!(cell.c, ' ' | '\0' | '\t') && !style.underline && !style.strikethrough;

            let extends_last = self.batches[first_batch..].last().is_some_and(|batch| {
                !batch.isolated && !isolated && batch.column + batch.cells == column && batch.style == style
            });
            if extends_last {
                let batch = self.batches.last_mut().unwrap();
                batch.text.push(if blank { ' ' } else { cell.c });
                batch.cells += 1;
            } else if !blank {
                let mut text = String::from(cell.c);
                text.extend(zero_width.into_iter().flatten());
                self.batches.push(TextBatch { y, column, cells, text, style, isolated });
            }
        }

        // Cursor past the last cell of the row (a short frozen row).
        if let Some((column, shape)) = cursor
            && !cursor_on_cell
        {
            self.cursor = Some(CursorPaint { y, column, cells: 1, shape });
        }
    }

    /// The rectangle occupied by the terminal cursor, if it is in this layer.
    pub fn cursor_bounds(&self, origin_x: Pixels, metrics: &Metrics) -> Option<Bounds<Pixels>> {
        let cursor = self.cursor.as_ref()?;
        Some(Bounds::new(
            point(origin_x + metrics.cell_width * cursor.column as f32, cursor.y),
            size(metrics.cell_width * cursor.cells as f32, metrics.cell_height),
        ))
    }

    /// Draws the layer. `origin_x` is the left edge of column 0.
    pub fn paint(
        &self,
        origin_x: Pixels,
        metrics: &Metrics,
        palette: &Palette,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Metrics { font, font_size, cell_width, cell_height } = metrics.clone();
        let cell_bounds = |y: Pixels, column: usize, cells: usize| {
            Bounds::new(
                point(origin_x + cell_width * column as f32, y),
                size(cell_width * cells as f32, cell_height),
            )
        };
        let text_system = window.text_system().clone();

        for quad in &self.quads_below {
            window.paint_quad(quad.clone());
        }
        for run in &self.backgrounds {
            window.paint_quad(fill(cell_bounds(run.y, run.column, run.cells), hsla(run.color)));
        }

        let cursor_color = hsla(palette.cursor());
        let cursor_bounds = self.cursor_bounds(origin_x, metrics);
        if let (Some(cursor), Some(bounds)) = (&self.cursor, cursor_bounds)
            && cursor.shape == CursorShape::Block
        {
            window.paint_quad(fill(bounds, cursor_color));
        }

        for batch in &self.batches {
            let text = batch.text.trim_end_matches(' ');
            if text.is_empty() {
                continue;
            }
            let color = hsla(batch.style.foreground);
            let mut run_font = font.clone();
            if batch.style.bold {
                run_font.weight = FontWeight::BOLD;
            }
            if batch.style.italic {
                run_font.style = FontStyle::Italic;
            }
            let run = TextRun {
                len: text.len(),
                font: run_font,
                color,
                background_color: None,
                underline: batch.style.underline.then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                }),
                strikethrough: batch.style.strikethrough.then_some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(color),
                }),
            };
            // `force_width` snaps each glyph to its cell; we do not apply it
            // to wide or combining characters, which overflow a cell.
            let force_width = (!batch.isolated).then_some(cell_width);
            let line = text_system.shape_line(text.to_owned().into(), font_size, &[run], force_width);
            let origin = cell_bounds(batch.y, batch.column, 1).origin;
            let _ = line.paint(origin, cell_height, window, cx);
        }

        for label in &self.labels {
            if label.text.is_empty() {
                continue;
            }
            let mut run_font = font.clone();
            if label.bold {
                run_font.weight = FontWeight::BOLD;
            }
            let run = TextRun {
                len: label.text.len(),
                font: run_font,
                color: label.color,
                background_color: None,
                underline: label.underline.then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(label.color),
                    wavy: false,
                }),
                strikethrough: None,
            };
            let force_width = label.on_grid.then_some(cell_width);
            let line = text_system.shape_line(label.text.clone().into(), font_size, &[run], force_width);
            let _ = line.paint(label.origin, cell_height, window, cx);
        }

        if let (Some(cursor), Some(bounds)) = (&self.cursor, cursor_bounds) {
            match cursor.shape {
                CursorShape::Beam => {
                    let beam = Bounds::new(bounds.origin, size(px(2.), bounds.size.height));
                    window.paint_quad(fill(beam, cursor_color));
                }
                CursorShape::Underline => {
                    let line = Bounds::new(
                        point(bounds.origin.x, bounds.origin.y + bounds.size.height - px(2.)),
                        size(bounds.size.width, px(2.)),
                    );
                    window.paint_quad(fill(line, cursor_color));
                }
                CursorShape::HollowBlock => {
                    window.paint_quad(outline(bounds, cursor_color, gpui::BorderStyle::Solid));
                }
                CursorShape::Block | CursorShape::Hidden => {}
            }
        }
        for quad in &self.quads_above {
            window.paint_quad(quad.clone());
        }
    }
}

/// A complete frame: the block area, clipped to its bounds, and what is
/// drawn on top without clipping (the input editor).
#[derive(Default)]
pub struct Scene {
    pub clip: Bounds<Pixels>,
    pub clipped: Layer,
    pub free: Layer,
}

impl Scene {
    pub fn paint(
        &self,
        origin_x: Pixels,
        metrics: &Metrics,
        palette: &Palette,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(ContentMask { bounds: self.clip }), |window| {
            self.clipped.paint(origin_x, metrics, palette, window, cx);
        });
        self.free.paint(origin_x, metrics, palette, window, cx);
    }
}
