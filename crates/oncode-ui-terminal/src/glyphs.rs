//! Characters that are drawn by us instead of taken from the font: box drawing lines, blocks,
//! and powerline separators. Font glyphs for these characters rarely fit the cell size (especially
//! when the line spacing is taller than the font), so vertical lines come out dashed and prompt
//! arrows do not fill the line height. Drawn as shapes that exactly fill the cell, these characters
//! join seamlessly with their neighboring cells, as in VS Code (xterm.js) and kitty.
//!
//! Pure (no GPUI): produces shapes in pixels relative to the cell's top-left corner.

/// A single step of a path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    /// A quadratic curve to `to` with control point `ctrl`.
    CurveTo { to: (f32, f32), ctrl: (f32, f32) },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A filled rectangle; `alpha` < 1 for shading (░ ▒ ▓).
    Rect { x: f32, y: f32, w: f32, h: f32, alpha: f32 },
    /// A closed filled area.
    Fill(Vec<Segment>),
    /// A line along the path with thickness `width`.
    Stroke { path: Vec<Segment>, width: f32 },
}

/// Cell size and line thickness, already rounded to device pixels by the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub w: f32,
    pub h: f32,
    /// Thickness of a light line; a heavy line is twice that.
    pub light: f32,
    /// The screen scale factor, for rounding positions to device pixels.
    pub scale: f32,
}

impl Cell {
    fn snap(&self, value: f32) -> f32 {
        (value * self.scale).round() / self.scale
    }

    fn heavy(&self) -> f32 {
        self.light * 2.
    }

    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
        let (x0, y0, x1, y1) = (self.snap(x0), self.snap(y0), self.snap(x1), self.snap(y1));
        Shape::Rect { x: x0, y: y0, w: (x1 - x0).max(0.), h: (y1 - y0).max(0.), alpha: 1. }
    }

    /// A part of the cell, in fractions of the width/height.
    fn part(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
        self.rect(x0 * self.w, y0 * self.h, x1 * self.w, y1 * self.h)
    }
}

/// This character is drawn by us (see [`shapes`]).
pub fn is_custom(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x259F | 0xE0B0..=0xE0B7)
}

/// The shapes for this character, or `None` if this character is drawn with the font.
pub fn shapes(c: char, cell: Cell) -> Option<Vec<Shape>> {
    let code = c as u32;
    match code {
        0x2500..=0x254B | 0x2574..=0x257F => match dashes(code) {
            Some((vertical, count, heavy)) => Some(dashed(cell, vertical, count, heavy)),
            None => Some(arms(cell, ARMS[arm_index(code)?])),
        },
        0x254C..=0x254F => {
            let (vertical, count, heavy) = dashes(code)?;
            Some(dashed(cell, vertical, count, heavy))
        }
        0x2550..=0x256C => Some(double(cell, DOUBLE[(code - 0x2550) as usize])),
        0x256D..=0x2570 => Some(vec![arc(cell, code)]),
        0x2571..=0x2573 => Some(diagonals(cell, code)),
        0x2580..=0x259F => Some(block(cell, code)),
        0xE0B0..=0xE0B7 => Some(powerline(cell, code)),
        _ => None,
    }
}

// ----- Light and heavy lines -----

/// The arms (up, right, down, left) of each character: 0 none, 1 light, 2 heavy.
/// Index 0..=0x4B for U+2500..=U+254B, then 0x4C.. for U+2574..=U+257F.
const ARMS: [[u8; 4]; 88] = [
    [0, 1, 0, 1], // ─
    [0, 2, 0, 2], // ━
    [1, 0, 1, 0], // │
    [2, 0, 2, 0], // ┃
    [0, 0, 0, 0], // ┄ (dashed, see `dashes`)
    [0, 0, 0, 0], // ┅
    [0, 0, 0, 0], // ┆
    [0, 0, 0, 0], // ┇
    [0, 0, 0, 0], // ┈
    [0, 0, 0, 0], // ┉
    [0, 0, 0, 0], // ┊
    [0, 0, 0, 0], // ┋
    [0, 1, 1, 0], // ┌
    [0, 2, 1, 0], // ┍
    [0, 1, 2, 0], // ┎
    [0, 2, 2, 0], // ┏
    [0, 0, 1, 1], // ┐
    [0, 0, 1, 2], // ┑
    [0, 0, 2, 1], // ┒
    [0, 0, 2, 2], // ┓
    [1, 1, 0, 0], // └
    [1, 2, 0, 0], // ┕
    [2, 1, 0, 0], // ┖
    [2, 2, 0, 0], // ┗
    [1, 0, 0, 1], // ┘
    [1, 0, 0, 2], // ┙
    [2, 0, 0, 1], // ┚
    [2, 0, 0, 2], // ┛
    [1, 1, 1, 0], // ├
    [1, 2, 1, 0], // ┝
    [2, 1, 1, 0], // ┞
    [1, 1, 2, 0], // ┟
    [2, 1, 2, 0], // ┠
    [2, 2, 1, 0], // ┡
    [1, 2, 2, 0], // ┢
    [2, 2, 2, 0], // ┣
    [1, 0, 1, 1], // ┤
    [1, 0, 1, 2], // ┥
    [2, 0, 1, 1], // ┦
    [1, 0, 2, 1], // ┧
    [2, 0, 2, 1], // ┨
    [2, 0, 1, 2], // ┩
    [1, 0, 2, 2], // ┪
    [2, 0, 2, 2], // ┫
    [0, 1, 1, 1], // ┬
    [0, 1, 1, 2], // ┭
    [0, 2, 1, 1], // ┮
    [0, 2, 1, 2], // ┯
    [0, 1, 2, 1], // ┰
    [0, 1, 2, 2], // ┱
    [0, 2, 2, 1], // ┲
    [0, 2, 2, 2], // ┳
    [1, 1, 0, 1], // ┴
    [1, 1, 0, 2], // ┵
    [1, 2, 0, 1], // ┶
    [1, 2, 0, 2], // ┷
    [2, 1, 0, 1], // ┸
    [2, 1, 0, 2], // ┹
    [2, 2, 0, 1], // ┺
    [2, 2, 0, 2], // ┻
    [1, 1, 1, 1], // ┼
    [1, 1, 1, 2], // ┽
    [1, 2, 1, 1], // ┾
    [1, 2, 1, 2], // ┿
    [2, 1, 1, 1], // ╀
    [1, 1, 2, 1], // ╁
    [2, 1, 2, 1], // ╂
    [2, 1, 1, 2], // ╃
    [2, 2, 1, 1], // ╄
    [1, 1, 2, 2], // ╅
    [1, 2, 2, 1], // ╆
    [2, 2, 1, 2], // ╇
    [1, 2, 2, 2], // ╈
    [2, 1, 2, 2], // ╉
    [2, 2, 2, 1], // ╊
    [2, 2, 2, 2], // ╋
    [0, 0, 0, 1], // ╴
    [1, 0, 0, 0], // ╵
    [0, 1, 0, 0], // ╶
    [0, 0, 1, 0], // ╷
    [0, 0, 0, 2], // ╸
    [2, 0, 0, 0], // ╹
    [0, 2, 0, 0], // ╺
    [0, 0, 2, 0], // ╻
    [0, 2, 0, 1], // ╼
    [1, 0, 2, 0], // ╽
    [0, 1, 0, 2], // ╾
    [2, 0, 1, 0], // ╿
];

fn arm_index(code: u32) -> Option<usize> {
    match code {
        0x2500..=0x254B => Some((code - 0x2500) as usize),
        0x2574..=0x257F => Some((code - 0x2574) as usize + 0x4C),
        _ => None,
    }
}

/// Lines from the center of the cell to each side that has an arm. An arm extends slightly past the
/// center (by the thickness of its perpendicular line) so corners and junctions close up tightly.
fn arms(cell: Cell, [up, right, down, left]: [u8; 4]) -> Vec<Shape> {
    let thick = |weight: u8| if weight == 2 { cell.heavy() } else { cell.light };
    let widest = |a: u8, b: u8| match a.max(b) {
        0 => None,
        weight => Some(thick(weight)),
    };
    // The left edge of the vertical line and the top edge of the horizontal line (the thickest one) at the center of the cell.
    let vertical = widest(up, down).map(|t| (cell.snap((cell.w - t) / 2.), t));
    let horizontal = widest(left, right).map(|t| (cell.snap((cell.h - t) / 2.), t));
    let (mid_x, mid_y) = (cell.snap(cell.w / 2.), cell.snap(cell.h / 2.));
    let mut shapes = Vec::new();
    for (weight, toward_end) in [(left, false), (right, true)] {
        if weight == 0 {
            continue;
        }
        let t = thick(weight);
        let y = cell.snap((cell.h - t) / 2.);
        let (x0, x1) = match (toward_end, vertical) {
            (false, Some((x, vt))) => (0., x + vt),
            (false, None) => (0., mid_x),
            (true, Some((x, _))) => (x, cell.w),
            (true, None) => (mid_x, cell.w),
        };
        shapes.push(cell.rect(x0, y, x1, y + t));
    }
    for (weight, toward_end) in [(up, false), (down, true)] {
        if weight == 0 {
            continue;
        }
        let t = thick(weight);
        let x = cell.snap((cell.w - t) / 2.);
        let (y0, y1) = match (toward_end, horizontal) {
            (false, Some((y, ht))) => (0., y + ht),
            (false, None) => (0., mid_y),
            (true, Some((y, _))) => (y, cell.h),
            (true, None) => (mid_y, cell.h),
        };
        shapes.push(cell.rect(x, y0, x + t, y1));
    }
    shapes
}

/// Dashed line: (vertical?, number of segments, heavy?).
fn dashes(code: u32) -> Option<(bool, usize, bool)> {
    Some(match code {
        0x2504 => (false, 3, false),
        0x2505 => (false, 3, true),
        0x2506 => (true, 3, false),
        0x2507 => (true, 3, true),
        0x2508 => (false, 4, false),
        0x2509 => (false, 4, true),
        0x250A => (true, 4, false),
        0x250B => (true, 4, true),
        0x254C => (false, 2, false),
        0x254D => (false, 2, true),
        0x254E => (true, 2, false),
        0x254F => (true, 2, true),
        _ => return None,
    })
}

fn dashed(cell: Cell, vertical: bool, count: usize, heavy: bool) -> Vec<Shape> {
    let t = if heavy { cell.heavy() } else { cell.light };
    let length = if vertical { cell.h } else { cell.w };
    let segment = length / count as f32;
    // A gap at both ends of each segment, so the pattern repeats evenly across cells.
    let gap = segment * 0.2;
    (0..count)
        .map(|i| {
            let (a, b) = (i as f32 * segment + gap, (i + 1) as f32 * segment - gap);
            if vertical {
                let x = cell.snap((cell.w - t) / 2.);
                cell.rect(x, a, x + t, b)
            } else {
                let y = cell.snap((cell.h - t) / 2.);
                cell.rect(a, y, b, y + t)
            }
        })
        .collect()
}

// ----- Double lines -----

/// A position along the cell axis: 0 the start edge, 1 the first double line, 2 the center, 3 the
/// second double line, 4 the end edge.
type Pos = u8;

/// A line segment: (horizontal?, cross position, from, to).
type Line = (bool, Pos, Pos, Pos);

const H: bool = true;
const V: bool = false;

/// The segments of each character U+2550..=U+256C (double lines and their mixes with single lines).
const DOUBLE: [&[Line]; 29] = [
    &[(H, 1, 0, 4), (H, 3, 0, 4)],                                                                   // ═
    &[(V, 1, 0, 4), (V, 3, 0, 4)],                                                                   // ║
    &[(H, 1, 2, 4), (H, 3, 2, 4), (V, 2, 1, 4)],                                                     // ╒
    &[(H, 2, 1, 4), (V, 1, 2, 4), (V, 3, 2, 4)],                                                     // ╓
    &[(H, 1, 1, 4), (H, 3, 3, 4), (V, 1, 1, 4), (V, 3, 3, 4)],                                       // ╔
    &[(H, 1, 0, 2), (H, 3, 0, 2), (V, 2, 1, 4)],                                                     // ╕
    &[(H, 2, 0, 3), (V, 1, 2, 4), (V, 3, 2, 4)],                                                     // ╖
    &[(H, 1, 0, 3), (H, 3, 0, 1), (V, 3, 1, 4), (V, 1, 3, 4)],                                       // ╗
    &[(H, 1, 2, 4), (H, 3, 2, 4), (V, 2, 0, 3)],                                                     // ╘
    &[(H, 2, 1, 4), (V, 1, 0, 2), (V, 3, 0, 2)],                                                     // ╙
    &[(H, 3, 1, 4), (H, 1, 3, 4), (V, 1, 0, 3), (V, 3, 0, 1)],                                       // ╚
    &[(H, 1, 0, 2), (H, 3, 0, 2), (V, 2, 0, 3)],                                                     // ╛
    &[(H, 2, 0, 3), (V, 1, 0, 2), (V, 3, 0, 2)],                                                     // ╜
    &[(H, 3, 0, 3), (H, 1, 0, 1), (V, 3, 0, 3), (V, 1, 0, 1)],                                       // ╝
    &[(V, 2, 0, 4), (H, 1, 2, 4), (H, 3, 2, 4)],                                                     // ╞
    &[(V, 1, 0, 4), (V, 3, 0, 4), (H, 2, 3, 4)],                                                     // ╟
    &[(V, 1, 0, 4), (V, 3, 0, 1), (V, 3, 3, 4), (H, 1, 3, 4), (H, 3, 3, 4)],                         // ╠
    &[(V, 2, 0, 4), (H, 1, 0, 2), (H, 3, 0, 2)],                                                     // ╡
    &[(V, 1, 0, 4), (V, 3, 0, 4), (H, 2, 0, 1)],                                                     // ╢
    &[(V, 3, 0, 4), (V, 1, 0, 1), (V, 1, 3, 4), (H, 1, 0, 1), (H, 3, 0, 1)],                         // ╣
    &[(H, 1, 0, 4), (H, 3, 0, 4), (V, 2, 3, 4)],                                                     // ╤
    &[(H, 2, 0, 4), (V, 1, 2, 4), (V, 3, 2, 4)],                                                     // ╥
    &[(H, 1, 0, 4), (H, 3, 0, 1), (H, 3, 3, 4), (V, 1, 3, 4), (V, 3, 3, 4)],                         // ╦
    &[(H, 1, 0, 4), (H, 3, 0, 4), (V, 2, 0, 1)],                                                     // ╧
    &[(H, 2, 0, 4), (V, 1, 0, 2), (V, 3, 0, 2)],                                                     // ╨
    &[(H, 3, 0, 4), (H, 1, 0, 1), (H, 1, 3, 4), (V, 1, 0, 1), (V, 3, 0, 1)],                         // ╩
    &[(H, 1, 0, 4), (H, 3, 0, 4), (V, 2, 0, 4)],                                                     // ╪
    &[(V, 1, 0, 4), (V, 3, 0, 4), (H, 2, 0, 4)],                                                     // ╫
    &[(H, 1, 0, 1), (H, 1, 3, 4), (H, 3, 0, 1), (H, 3, 3, 4), (V, 1, 0, 1), (V, 1, 3, 4), (V, 3, 0, 1), (V, 3, 3, 4)], // ╬
];

fn double(cell: Cell, lines: &[Line]) -> Vec<Shape> {
    let t = cell.light;
    // The distance of each double line from the center: the gap between the two is one line thick.
    let at = |pos: Pos, length: f32| {
        let mid = cell.snap((length - t) / 2.) + t / 2.;
        match pos {
            0 => 0.,
            1 => mid - t,
            2 => mid,
            3 => mid + t,
            _ => length,
        }
    };
    lines
        .iter()
        .map(|&(horizontal, across, from, to)| {
            let (along, cross) = if horizontal { (cell.w, cell.h) } else { (cell.h, cell.w) };
            // Ends inside the cell are extended by half the line thickness so corners close up.
            let start = if from == 0 { 0. } else { at(from, along) - t / 2. };
            let end = if to == 4 { along } else { at(to, along) + t / 2. };
            let center = at(across, cross);
            if horizontal {
                cell.rect(start, center - t / 2., end, center + t / 2.)
            } else {
                cell.rect(center - t / 2., start, center + t / 2., end)
            }
        })
        .collect()
}

// ----- Arcs and diagonals -----

fn arc(cell: Cell, code: u32) -> Shape {
    let t = cell.light;
    // The center of the line, same as the straight line in the neighboring cell.
    let cx = cell.snap((cell.w - t) / 2.) + t / 2.;
    let cy = cell.snap((cell.h - t) / 2.) + t / 2.;
    let r = (cell.w / 2.).min(cell.h / 2.);
    let (down, right) = match code {
        0x256D => (true, true),   // ╭
        0x256E => (true, false),  // ╮
        0x256F => (false, false), // ╯
        _ => (false, true),       // ╰
    };
    let (edge_y, bend_y) = if down { (cell.h, cy + r) } else { (0., cy - r) };
    let (edge_x, bend_x) = if right { (cell.w, cx + r) } else { (0., cx - r) };
    Shape::Stroke {
        path: vec![
            Segment::MoveTo(cx, edge_y),
            Segment::LineTo(cx, bend_y),
            Segment::CurveTo { to: (bend_x, cy), ctrl: (cx, cy) },
            Segment::LineTo(edge_x, cy),
        ],
        width: t,
    }
}

fn diagonals(cell: Cell, code: u32) -> Vec<Shape> {
    let (w, h, t) = (cell.w, cell.h, cell.light);
    let rising = Shape::Stroke { path: vec![Segment::MoveTo(w, 0.), Segment::LineTo(0., h)], width: t };
    let falling = Shape::Stroke { path: vec![Segment::MoveTo(0., 0.), Segment::LineTo(w, h)], width: t };
    match code {
        0x2571 => vec![rising],
        0x2572 => vec![falling],
        _ => vec![rising, falling],
    }
}

// ----- Blocks -----

fn block(cell: Cell, code: u32) -> Vec<Shape> {
    let eighth = |n: u32| n as f32 / 8.;
    match code {
        0x2580 => vec![cell.part(0., 0., 1., 0.5)],
        0x2581..=0x2587 => vec![cell.part(0., 1. - eighth(code - 0x2580), 1., 1.)],
        0x2588 => vec![cell.part(0., 0., 1., 1.)],
        0x2589..=0x258F => vec![cell.part(0., 0., eighth(0x2590 - code), 1.)],
        0x2590 => vec![cell.part(0.5, 0., 1., 1.)],
        0x2591..=0x2593 => {
            // Shading as a transparent color (not a dot pattern).
            let alpha = (code - 0x2590) as f32 * 0.25;
            let Shape::Rect { x, y, w, h, .. } = cell.part(0., 0., 1., 1.) else { unreachable!() };
            vec![Shape::Rect { x, y, w, h, alpha }]
        }
        0x2594 => vec![cell.part(0., 0., 1., eighth(1))],
        0x2595 => vec![cell.part(eighth(7), 0., 1., 1.)],
        _ => {
            // Quadrants: upper left, upper right, lower left, lower right.
            let [ul, ur, ll, lr] = match code {
                0x2596 => [false, false, true, false],
                0x2597 => [false, false, false, true],
                0x2598 => [true, false, false, false],
                0x2599 => [true, false, true, true],
                0x259A => [true, false, false, true],
                0x259B => [true, true, true, false],
                0x259C => [true, true, false, true],
                0x259D => [false, true, false, false],
                0x259E => [false, true, true, false],
                _ => [false, true, true, true],
            };
            [(ul, 0., 0.), (ur, 0.5, 0.), (ll, 0., 0.5), (lr, 0.5, 0.5)]
                .into_iter()
                .filter(|(on, _, _)| *on)
                .map(|(_, x, y)| cell.part(x, y, x + 0.5, y + 0.5))
                .collect()
        }
    }
}

// ----- Powerline -----

fn powerline(cell: Cell, code: u32) -> Vec<Shape> {
    let (w, h, t) = (cell.w, cell.h, cell.light);
    let mid = h / 2.;
    match code {
        // A solid triangle pointing right / left.
        0xE0B0 => vec![Shape::Fill(vec![Segment::MoveTo(0., 0.), Segment::LineTo(w, mid), Segment::LineTo(0., h)])],
        0xE0B2 => vec![Shape::Fill(vec![Segment::MoveTo(w, 0.), Segment::LineTo(0., mid), Segment::LineTo(w, h)])],
        // A thin arrow.
        0xE0B1 => vec![Shape::Stroke { path: vec![Segment::MoveTo(0., 0.), Segment::LineTo(w - t / 2., mid), Segment::LineTo(0., h)], width: t }],
        0xE0B3 => vec![Shape::Stroke { path: vec![Segment::MoveTo(w, 0.), Segment::LineTo(t / 2., mid), Segment::LineTo(w, h)], width: t }],
        // A solid half circle / a curved line.
        0xE0B4 => vec![Shape::Fill(half_round(cell, true))],
        0xE0B6 => vec![Shape::Fill(half_round(cell, false))],
        0xE0B5 => vec![Shape::Stroke { path: half_round(cell, true), width: t }],
        _ => vec![Shape::Stroke { path: half_round(cell, false), width: t }],
    }
}

/// A half circle that bulges to the right (`right`) or to the left, as tall as the cell.
fn half_round(cell: Cell, right: bool) -> Vec<Segment> {
    let (w, h) = (cell.w, cell.h);
    let (flat, bulge) = if right { (0., w) } else { (w, 0.) };
    vec![
        Segment::MoveTo(flat, 0.),
        Segment::CurveTo { to: (bulge, h / 2.), ctrl: (bulge, 0.) },
        Segment::CurveTo { to: (flat, h), ctrl: (bulge, h) },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: Cell = Cell { w: 8., h: 17., light: 1., scale: 1. };

    fn rects(c: char) -> Vec<(f32, f32, f32, f32)> {
        shapes(c, CELL)
            .unwrap()
            .into_iter()
            .map(|shape| match shape {
                Shape::Rect { x, y, w, h, .. } => (x, y, w, h),
                other => panic!("not a rectangle: {other:?}"),
            })
            .collect()
    }

    /// The area covered by these shapes includes the point (x, y).
    fn covers(c: char, x: f32, y: f32) -> bool {
        rects(c).iter().any(|(rx, ry, rw, rh)| x >= *rx && x < rx + rw && y >= *ry && y < ry + rh)
    }

    #[test]
    fn ordinary_characters_use_the_font() {
        for c in ['a', ' ', '-', '|', '=', '漢', '\u{E0A0}'] {
            assert!(shapes(c, CELL).is_none(), "{c:?}");
        }
    }

    #[test]
    fn straight_lines_reach_both_edges_so_neighbours_connect() {
        let total_width: f32 = rects('─').iter().map(|r| r.2).sum();
        assert_eq!(total_width, CELL.w);
        assert!(covers('─', 0., 8.) && covers('─', 7.9, 8.));
        let total_height: f32 = rects('│').iter().map(|r| r.3).sum();
        assert_eq!(total_height, CELL.h, "the vertical line is as tall as the line, with no gap between lines");
        assert!(covers('│', 4., 0.) && covers('│', 4., 16.9));
    }

    #[test]
    fn corners_and_junctions_meet_without_a_notch() {
        // Vertical line at x 4..5, horizontal at y 8..9 (cell 8×17, 1 px line).
        // ┌: the right and down arms meet at the center, the corner is closed.
        assert!(covers('┌', 4.5, 8.) && covers('┌', 7.9, 8.) && covers('┌', 4.5, 16.9));
        assert!(!covers('┌', 0., 8.) && !covers('┌', 4.5, 0.));
        // ┼: all four sides.
        assert!(covers('┼', 0., 8.) && covers('┼', 7.9, 8.) && covers('┼', 4.5, 0.) && covers('┼', 4.5, 16.9));
        // ━ is twice as thick as ─.
        assert_eq!(rects('━')[0].3, 2. * rects('─')[0].3);
        // Half line: ╴ is left only.
        assert!(covers('╴', 0., 8.) && !covers('╴', 7.9, 8.));
    }

    #[test]
    fn table_matches_the_unicode_names() {
        // Samples from the parts that are easy to get out of order.
        assert_eq!(ARMS[arm_index('┞' as u32).unwrap()], [2, 1, 1, 0], "up heavy and right down light");
        assert_eq!(ARMS[arm_index('╆' as u32).unwrap()], [1, 2, 2, 1], "right down heavy and left up light");
        assert_eq!(ARMS[arm_index('╋' as u32).unwrap()], [2, 2, 2, 2]);
        assert_eq!(ARMS[arm_index('╿' as u32).unwrap()], [2, 0, 1, 0], "heavy up and light down");
        assert_eq!(DOUBLE.len(), 0x256C - 0x2550 + 1);
    }

    #[test]
    fn double_lines_are_two_parallel_strokes() {
        let lines = rects('═');
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|r| r.0 == 0. && r.2 == CELL.w));
        assert!(lines[1].1 - (lines[0].1 + lines[0].3) >= 1., "there is a gap between the two");
        // ╔: two nested corners; the outer upper-left part of the cell is empty.
        assert!(!covers('╔', 0., 0.) && covers('╔', 7.9, lines[0].1) && covers('╔', 7.9, lines[1].1));
    }

    #[test]
    fn dashed_lines_have_gaps() {
        let dashes = rects('┄');
        assert_eq!(dashes.len(), 3);
        assert!(dashes.windows(2).all(|pair| pair[1].0 > pair[0].0 + pair[0].2));
        assert_eq!(rects('┆').len(), 3);
        assert_eq!(rects('╌').len(), 2);
    }

    #[test]
    fn blocks_fill_their_part_of_the_cell() {
        assert_eq!(rects('█'), [(0., 0., 8., 17.)]);
        assert_eq!(rects('▌'), [(0., 0., 4., 17.)]);
        assert_eq!(rects('▀')[0].1, 0.);
        assert!(covers('▄', 4., 16.9) && !covers('▄', 4., 1.));
        assert_eq!(rects('▚').len(), 2);
        assert!(matches!(shapes('▒', CELL).unwrap()[0], Shape::Rect { alpha, .. } if alpha == 0.5));
    }

    #[test]
    fn powerline_separators_span_the_full_row() {
        assert_eq!(
            shapes('\u{E0B0}', CELL).unwrap(),
            [Shape::Fill(vec![Segment::MoveTo(0., 0.), Segment::LineTo(8., 8.5), Segment::LineTo(0., 17.)])],
            "a solid triangle as tall as the cell"
        );
        assert!(matches!(shapes('\u{E0B1}', CELL).unwrap()[0], Shape::Stroke { .. }));
        assert!(matches!(shapes('╭', CELL).unwrap()[0], Shape::Stroke { .. }));
    }

    #[test]
    fn positions_snap_to_device_pixels() {
        let retina = Cell { w: 8.4, h: 17., light: 1., scale: 2. };
        for shape in shapes('│', retina).unwrap() {
            let Shape::Rect { x, w, .. } = shape else { panic!() };
            assert_eq!((x * 2.).fract(), 0.);
            assert_eq!((w * 2.).fract(), 0.);
        }
    }
}
