//! Menggambar isi layar terminal (`grid::Frame`) ke jendela: latar per sel, seleksi, karakter
//! garis/blok/powerline sebagai bentuk, teks per potongan dengan lebar huruf dipaksa selebar sel,
//! lalu kursor.

use std::sync::Arc;

use alacritty_terminal::vte::ansi::CursorShape;
use gpui_kit::{
    fill, outline, point, px, size, App, BorderStyle, Bounds, Font, FontFeatures, FontStyle, FontWeight, Hsla, PathBuilder, Pixels, Point, SharedString,
    StrikethroughStyle, TextAlign, TextRun, UnderlineStyle, Window,
};

use crate::glyphs::{self, Segment, Shape};
use crate::grid::Frame;
use crate::palette::{to_hsla, Palette};

/// Font terminal tanpa ligatur: `->`, `!=`, `=>` tetap dua huruf di dua sel, seperti terminal lain.
pub fn terminal_font(family: SharedString) -> Font {
    Font {
        family,
        features: FontFeatures(Arc::new(vec![("calt".into(), 0), ("liga".into(), 0)])),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

/// Ukuran yang dipakai untuk menggambar; sudah dibulatkan ke piksel perangkat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    pub cell_width: Pixels,
    pub line_height: Pixels,
    /// Ketebalan garis tipis (box drawing, garis bawah, kursor).
    pub line: f32,
    pub scale: f32,
}

impl CellMetrics {
    fn bounds(&self, origin: Point<Pixels>, row: usize, column: usize, cells: usize) -> Bounds<Pixels> {
        Bounds::new(point(origin.x + self.cell_width * column as f32, origin.y + self.line_height * row as f32), size(self.cell_width * cells as f32, self.line_height))
    }
}

pub fn paint_frame(frame: &Frame, origin: Point<Pixels>, metrics: CellMetrics, font: &Font, font_size: Pixels, palette: &Palette, window: &mut Window, cx: &mut App) {
    // Titik awal di batas piksel perangkat, agar tepi sel tidak jatuh di tengah piksel.
    let snap = |value: Pixels| px((f32::from(value) * metrics.scale).round() / metrics.scale);
    let origin = point(snap(origin.x), snap(origin.y));

    for span in &frame.backgrounds {
        window.paint_quad(fill(metrics.bounds(origin, span.row, span.start, span.end - span.start), to_hsla(span.color)));
    }
    let (selection, alpha) = palette.selection;
    for span in &frame.selections {
        window.paint_quad(fill(metrics.bounds(origin, span.row, span.start, span.end - span.start), to_hsla(selection).opacity(alpha)));
    }

    let cell = glyphs::Cell { w: f32::from(metrics.cell_width), h: f32::from(metrics.line_height), light: metrics.line, scale: metrics.scale };
    for glyph in &frame.glyphs {
        let Some(shapes) = glyphs::shapes(glyph.c, cell) else { continue };
        let at = metrics.bounds(origin, glyph.row, glyph.column, 1).origin;
        for shape in &shapes {
            paint_shape(shape, at, to_hsla(glyph.fg), window);
        }
    }

    for run in &frame.runs {
        let color = to_hsla(run.fg);
        let font = Font {
            weight: if run.bold { FontWeight::BOLD } else { FontWeight::NORMAL },
            style: if run.italic { FontStyle::Italic } else { FontStyle::Normal },
            ..font.clone()
        };
        let style = TextRun {
            len: run.text.len(),
            font,
            color,
            background_color: None,
            underline: run.underline.then(|| UnderlineStyle { thickness: px(metrics.line), color: Some(color), wavy: false }),
            strikethrough: run.strikethrough.then(|| StrikethroughStyle { thickness: px(metrics.line), color: Some(color) }),
        };
        // Setiap huruf ditaruh tepat di selnya, apa pun lebar glyph aslinya (font cadangan, ikon).
        let shaped = window.text_system().shape_line(SharedString::from(run.text.clone()), font_size, &[style], Some(metrics.cell_width * run.cell_width as f32));
        let at = metrics.bounds(origin, run.row, run.column, run.cells).origin;
        let _ = shaped.paint(at, metrics.line_height, TextAlign::Left, None, window, cx);
    }

    if let Some(cursor) = frame.cursor {
        let bounds = metrics.bounds(origin, cursor.row, cursor.column, cursor.cells);
        let color = to_hsla(cursor.color);
        let thick = px(metrics.line * 2.);
        match cursor.shape {
            // Latar dan warna huruf kursor blok sudah ada di `frame`.
            CursorShape::Block | CursorShape::Hidden => {}
            CursorShape::HollowBlock => window.paint_quad(outline(bounds, color, BorderStyle::default())),
            CursorShape::Beam => window.paint_quad(fill(Bounds::new(bounds.origin, size(thick, bounds.size.height)), color)),
            CursorShape::Underline => {
                window.paint_quad(fill(Bounds::new(point(bounds.origin.x, bounds.origin.y + bounds.size.height - thick), size(bounds.size.width, thick)), color))
            }
        }
    }
}

fn paint_shape(shape: &Shape, origin: Point<Pixels>, color: Hsla, window: &mut Window) {
    let at = |(x, y): (f32, f32)| point(origin.x + px(x), origin.y + px(y));
    let trace = |builder: &mut PathBuilder, path: &[Segment]| {
        for segment in path {
            match *segment {
                Segment::MoveTo(x, y) => builder.move_to(at((x, y))),
                Segment::LineTo(x, y) => builder.line_to(at((x, y))),
                Segment::CurveTo { to, ctrl } => builder.curve_to(at(to), at(ctrl)),
            }
        }
    };
    match shape {
        Shape::Rect { x, y, w, h, alpha } => window.paint_quad(fill(Bounds::new(at((*x, *y)), size(px(*w), px(*h))), color.opacity(*alpha))),
        Shape::Fill(path) => {
            let mut builder = PathBuilder::fill();
            trace(&mut builder, path);
            builder.close();
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        }
        Shape::Stroke { path, width } => {
            let mut builder = PathBuilder::stroke(px(*width));
            trace(&mut builder, path);
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        }
    }
}
