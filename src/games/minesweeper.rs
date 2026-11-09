//! Minesweeper helper. Reads the board off the screen, works out which tiles
//! are *provably* safe from the numbers showing, and every few tiles you
//! uncover, the stickman stomps one of them open for you.
//!
//! It can't lose the game for you: it ignores your flags (a wrong flag could
//! fool it), refuses to act on a board that doesn't make sense, re-checks
//! right before pressing, and only presses tiles the numbers prove are
//! safe. The one exception is the "Let it make mistakes" option.

use std::path::PathBuf;

use tiny_skia::{IntSize, Paint, Pixmap, Rect as SkRect, Transform};

use crate::math::{v2, Vec2};
use crate::platform::{Image, Platform};
use crate::rng::Rng;
use crate::world::WinId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Hidden,
    Flag,
    /// Uncovered, with this many mines around it.
    Open(u8),
    /// A revealed mine: the game is lost.
    Mine,
}

/// Which Minesweeper it is (they look completely different).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// Vista / 7 (blue glossy tiles, or the green flower-garden theme).
    Modern,
    /// XP's grey tiles.
    Xp,
}

#[derive(Clone, Debug)]
pub struct Board {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    /// Screen position of the grid's top-left corner, and tile size.
    pub origin: Vec2,
    pub pitch: Vec2,
}

impl Board {
    pub fn at(&self, c: usize, r: usize) -> Cell {
        self.cells[r * self.cols + c]
    }

    pub fn center(&self, c: usize, r: usize) -> Vec2 {
        self.origin + v2((c as f32 + 0.5) * self.pitch.x, (r as f32 + 0.5) * self.pitch.y)
    }

    /// Where a foot lands on top of a tile.
    pub fn top(&self, c: usize, r: usize) -> Vec2 {
        self.origin + v2((c as f32 + 0.5) * self.pitch.x, (r as f32 + 0.25) * self.pitch.y)
    }

    pub fn opened(&self) -> usize {
        self.cells.iter().filter(|c| matches!(c, Cell::Open(_))).count()
    }

    pub fn lost(&self) -> bool {
        self.cells.contains(&Cell::Mine)
    }

    fn neighbors(&self, c: usize, r: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::with_capacity(8);
        for dr in -1i32..=1 {
            for dc in -1i32..=1 {
                if dr == 0 && dc == 0 {
                    continue;
                }
                let (nc, nr) = (c as i32 + dc, r as i32 + dr);
                if nc >= 0 && nr >= 0 && (nc as usize) < self.cols && (nr as usize) < self.rows {
                    out.push((nc as usize, nr as usize));
                }
            }
        }
        out
    }

    /// False if the numbers can't be right (the board was misread).
    pub fn plausible(&self) -> bool {
        for r in 0..self.rows {
            for c in 0..self.cols {
                if let Cell::Open(n) = self.at(c, r) {
                    let covered = self
                        .neighbors(c, r)
                        .iter()
                        .filter(|&&(nc, nr)| matches!(self.at(nc, nr), Cell::Hidden | Cell::Flag))
                        .count();
                    if n as usize > covered {
                        return false;
                    }
                }
            }
        }
        true
    }
}

/// Every hidden, unflagged tile that the visible numbers prove is safe.
/// Flags are treated as unknown, so a wrong flag can't mislead it.
pub fn safe_cells(b: &Board) -> Vec<(usize, usize)> {
    let n = b.cols * b.rows;
    let idx = |c: usize, r: usize| r * b.cols + c;
    let unknown = |i: usize| matches!(b.cells[i], Cell::Hidden | Cell::Flag);
    let mut mine = vec![false; n];
    let mut safe = vec![false; n];

    loop {
        let mut changed = false;
        // Each number gives a constraint: these unknown tiles hold this many mines.
        let mut constraints: Vec<(Vec<usize>, i32)> = Vec::new();
        for r in 0..b.rows {
            for c in 0..b.cols {
                let Cell::Open(k) = b.at(c, r) else { continue };
                let mut unk = Vec::new();
                let mut mines = 0i32;
                for (nc, nr) in b.neighbors(c, r) {
                    let i = idx(nc, nr);
                    if !unknown(i) {
                        continue;
                    }
                    if mine[i] {
                        mines += 1;
                    } else if !safe[i] {
                        unk.push(i);
                    }
                }
                let left = k as i32 - mines;
                if unk.is_empty() || left < 0 {
                    continue;
                }
                if left == 0 {
                    unk.iter().for_each(|&i| safe[i] = true);
                    changed = true;
                } else if left == unk.len() as i32 {
                    unk.iter().for_each(|&i| mine[i] = true);
                    changed = true;
                } else {
                    constraints.push((unk, left));
                }
            }
        }
        // Subset rule: if A's tiles are all among B's, the rest of B holds
        // exactly (B - A) mines.
        if !changed {
            'outer: for a in 0..constraints.len() {
                for bi in 0..constraints.len() {
                    let (ua, ra) = &constraints[a];
                    let (ub, rb) = &constraints[bi];
                    if a == bi || ua.len() >= ub.len() || !ua.iter().all(|x| ub.contains(x)) {
                        continue;
                    }
                    let rest: Vec<usize> = ub.iter().filter(|x| !ua.contains(x)).copied().collect();
                    let d = rb - ra;
                    if d == 0 {
                        rest.iter().for_each(|&i| safe[i] = true);
                        changed = true;
                        break 'outer;
                    } else if d == rest.len() as i32 {
                        rest.iter().for_each(|&i| mine[i] = true);
                        changed = true;
                        break 'outer;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    (0..n).filter(|&i| safe[i] && b.cells[i] == Cell::Hidden).map(|i| (i % b.cols, i / b.cols)).collect()
}

// ---------------------------------------------------------------- reading

fn hsv(p: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-5 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max < 1e-5 { 0.0 } else { d / max };
    (h, s, max)
}

/// Covered tile: glossy blue (default theme) or green (flower garden).
fn modern_hidden(p: [u8; 3]) -> bool {
    let (h, s, v) = hsv(p);
    ((180.0..=245.0).contains(&h) && s > 0.30 && v > 0.40) || ((70.0..=150.0).contains(&h) && s > 0.35 && v > 0.35)
}

/// Uncovered tile background: pale and washed out.
fn modern_open(p: [u8; 3]) -> bool {
    let (_, s, v) = hsv(p);
    s < 0.25 && v > 0.62
}

fn redish(p: [u8; 3]) -> bool {
    let (h, s, v) = hsv(p);
    (h < 20.0 || h > 340.0) && s > 0.55 && v > 0.45
}

/// Which number an average digit colour is (the classic palette, shared by
/// every version): 1 blue, 2 green, 3 red, 4 navy, 5 maroon, 6 teal,
/// 7 black, 8 grey.
fn digit(avg: [u8; 3]) -> Option<u8> {
    let (h, s, v) = hsv(avg);
    if v < 0.22 {
        return Some(7);
    }
    if s < 0.2 {
        return (v < 0.65).then_some(8);
    }
    if (200.0..265.0).contains(&h) {
        return Some(if v > 0.55 { 1 } else { 4 });
    }
    if (75.0..170.0).contains(&h) {
        return Some(2);
    }
    if (170.0..200.0).contains(&h) {
        return Some(6);
    }
    if h < 25.0 || h > 330.0 {
        return Some(if v > 0.6 { 3 } else { 5 });
    }
    None
}

/// Look at the middle of a tile and decide what it is. `is_hidden`/`is_bg`
/// say which pixels are covered-tile and uncovered-background colours.
fn classify(
    img: &Image,
    x0: f32,
    y0: f32,
    w: f32,
    h: f32,
    is_hidden: impl Fn([u8; 3]) -> bool,
    is_bg: impl Fn([u8; 3]) -> bool,
) -> Option<Cell> {
    let (mut total, mut hidden, mut red) = (0u32, 0u32, 0u32);
    let mut ink = (0u32, [0u32; 3]);
    let steps = 8;
    for j in 0..steps {
        for i in 0..steps {
            let x = x0 + w * (0.25 + 0.5 * i as f32 / (steps - 1) as f32);
            let y = y0 + h * (0.25 + 0.5 * j as f32 / (steps - 1) as f32);
            let p = img.px(x as u32, y as u32);
            total += 1;
            if redish(p) {
                red += 1;
            }
            if is_hidden(p) {
                hidden += 1;
            } else if !is_bg(p) {
                ink.0 += 1;
                for k in 0..3 {
                    ink.1[k] += p[k] as u32;
                }
            }
        }
    }
    let frac = |n: u32| n as f32 / total as f32;
    if frac(red) > 0.45 {
        return Some(Cell::Mine);
    }
    if frac(hidden) > 0.55 {
        return Some(if frac(red) > 0.04 { Cell::Flag } else { Cell::Hidden });
    }
    if frac(ink.0) < 0.05 {
        return Some(Cell::Open(0));
    }
    let avg = [(ink.1[0] / ink.0) as u8, (ink.1[1] / ink.0) as u8, (ink.1[2] / ink.0) as u8];
    digit(avg).map(Cell::Open)
}

/// Find a periodic grid along one axis: the tile count whose boundaries
/// line up best with the dark gaps between tiles.
fn best_count(dark: &[f32], lo: usize, hi: usize, common: &[usize]) -> (usize, f32) {
    let len = dark.len() as f32;
    let mean = dark.iter().sum::<f32>() / len.max(1.0);
    let mut best = (0, f32::MIN);
    for c in lo..=hi {
        let pitch = len / c as f32;
        if pitch < 8.0 {
            break;
        }
        let mut sum = 0.0;
        for k in 1..c {
            let x = (k as f32 * pitch).round() as i32;
            let m = (x - 1..=x + 1)
                .filter(|&i| i >= 0 && (i as usize) < dark.len())
                .map(|i| dark[i as usize])
                .fold(f32::MIN, f32::max);
            sum += m;
        }
        let mut score = sum / (c - 1).max(1) as f32 - mean;
        if common.contains(&c) {
            score += 0.02;
        }
        if score > best.1 {
            best = (c, score);
        }
    }
    best
}

/// Longest run of indices where `f(i)` holds.
fn longest_run(n: usize, f: impl Fn(usize) -> bool) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    let mut start = None;
    for i in 0..=n {
        let on = i < n && f(i);
        match (on, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if best.map_or(true, |(a, b)| i - s > b - a) {
                    best = Some((s, i));
                }
                start = None;
            }
            _ => {}
        }
    }
    best
}

fn read_modern(img: &Image, origin: Vec2) -> Option<Board> {
    let (w, h) = (img.w as usize, img.h as usize);
    let tile = |x: usize, y: usize| {
        let p = img.px(x as u32, y as u32);
        modern_hidden(p) || modern_open(p)
    };
    // The board is the big block of tile-coloured pixels.
    let step = 2;
    let col_frac = |x: usize| (0..h).step_by(step).filter(|&y| tile(x, y)).count() as f32 / (h / step).max(1) as f32;
    let (x0, x1) = longest_run(w, |x| col_frac(x) > 0.3)?;
    let row_frac =
        |y: usize| (x0..x1).step_by(step).filter(|&x| tile(x, y)).count() as f32 / ((x1 - x0) / step).max(1) as f32;
    let (y0, y1) = longest_run(h, |y| row_frac(y) > 0.5)?;
    if x1 - x0 < 60 || y1 - y0 < 60 {
        return None;
    }
    // Darkness profiles (gaps between tiles are darker).
    let dark_x: Vec<f32> =
        (x0..x1).map(|x| (y0..y1).step_by(3).map(|y| 1.0 - hsv(img.px(x as u32, y as u32)).2).sum::<f32>()).collect();
    let dark_y: Vec<f32> =
        (y0..y1).map(|y| (x0..x1).step_by(3).map(|x| 1.0 - hsv(img.px(x as u32, y as u32)).2).sum::<f32>()).collect();
    let nx = dark_x.len().max(1) as f32;
    let ny = dark_y.len().max(1) as f32;
    let dark_x: Vec<f32> = dark_x.iter().map(|d| d / ny * 3.0).collect();
    let dark_y: Vec<f32> = dark_y.iter().map(|d| d / nx * 3.0).collect();
    let (cols, _) = best_count(&dark_x, 8, 30, &[9, 16, 30]);
    let (rows, _) = best_count(&dark_y, 8, 24, &[9, 16]);
    if cols == 0 || rows == 0 {
        return None;
    }
    let pitch = v2((x1 - x0) as f32 / cols as f32, (y1 - y0) as f32 / rows as f32);
    // Tiles are square.
    if (pitch.x - pitch.y).abs() / pitch.x > 0.2 || pitch.x < 10.0 || pitch.x > 90.0 {
        return None;
    }
    let mut cells = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        for c in 0..cols {
            let cx = x0 as f32 + c as f32 * pitch.x;
            let cy = y0 as f32 + r as f32 * pitch.y;
            cells.push(classify(img, cx, cy, pitch.x, pitch.y, modern_hidden, modern_open)?);
        }
    }
    Some(Board { cols, rows, cells, origin: origin + v2(x0 as f32, y0 as f32), pitch })
}

/// XP: fixed layout. The grid starts 12,55 px into the window and tiles
/// are 16 px, all scaled by `scale` when Windows stretches the old game.
fn read_xp(img: &Image, origin: Vec2, scale: f32) -> Option<Board> {
    let tile = 16.0 * scale;
    let (gx, gy) = (12.0 * scale, 55.0 * scale);
    let cols = ((img.w as f32 - gx - 8.0 * scale) / tile).round() as usize;
    let rows = ((img.h as f32 - gy - 8.0 * scale) / tile).round() as usize;
    if !(8..=30).contains(&cols) || !(8..=24).contains(&rows) {
        return None;
    }
    let white = |p: [u8; 3]| p.iter().all(|&c| c > 235);
    let grey_bg = |p: [u8; 3]| p.iter().all(|&c| (c as i32 - 192).abs() < 24) || white(p);
    let mut cells = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        for c in 0..cols {
            let x = gx + c as f32 * tile;
            let y = gy + r as f32 * tile;
            // Covered tiles have a white bevel along their top edge.
            let bevel = (2..14).filter(|&i| white(img.px((x + i as f32 * scale) as u32, (y + scale) as u32))).count();
            let cell = if bevel > 8 {
                // A red flag on a covered tile reads like a red digit.
                let flag = classify(img, x, y, tile, tile, |_| false, &grey_bg);
                if matches!(flag, Some(Cell::Open(3)) | Some(Cell::Open(5)) | Some(Cell::Mine)) {
                    Cell::Flag
                } else {
                    Cell::Hidden
                }
            } else {
                classify(img, x, y, tile, tile, |_| false, &grey_bg)?
            };
            cells.push(cell);
        }
    }
    Some(Board { cols, rows, cells, origin: origin + v2(gx, gy), pitch: v2(tile, tile) })
}

pub fn read(img: &Image, origin: Vec2, look: Look, scale: f32) -> Option<Board> {
    match look {
        Look::Modern => read_modern(img, origin),
        Look::Xp => read_xp(img, origin, scale),
    }
}

fn grab(plat: &mut dyn Platform, game: WinId) -> Option<(Image, Vec2)> {
    let cr = plat.client_rect(game)?;
    let img = plat.capture(cr)?;
    Some((img, v2(cr.l, cr.t)))
}

pub fn read_window(plat: &mut dyn Platform, game: WinId, look: Look) -> Option<Board> {
    let scale = plat.dpi_scale();
    let (img, origin) = grab(plat, game)?;
    read(&img, origin, look, scale)
}

// ---------------------------------------------------------------- helping

struct Pending {
    game: WinId,
    cell: (usize, usize),
    risky: bool,
    age: f32,
}

/// Decides when to help and which tile to press.
pub struct Helper {
    timer: f32,
    last_opened: usize,
    /// How many times you've uncovered something since it last helped.
    uncovers: u32,
    next_at: u32,
    /// The next increase in uncovered tiles is from our own click.
    ours: bool,
    pending: Option<Pending>,
}

impl Helper {
    pub fn new(rng: &mut Rng) -> Self {
        Helper { timer: 0.0, last_opened: 0, uncovers: 0, next_at: Self::gap(rng), ours: false, pending: None }
    }

    /// Help every 3 to 6 of your moves.
    fn gap(rng: &mut Rng) -> u32 {
        3 + (rng.f32() * 4.0) as u32
    }

    /// Call every frame while Minesweeper is in front. Returns a tile for the
    /// stickman to stomp when it's time to help.
    pub fn update(
        &mut self,
        dt: f32,
        plat: &mut dyn Platform,
        game: WinId,
        look: Look,
        ready: bool,
        mistakes: bool,
        rng: &mut Rng,
    ) -> Option<Vec2> {
        if let Some(p) = &mut self.pending {
            p.age += dt;
            if p.age > 4.0 {
                self.pending = None;
            }
            return None;
        }
        self.timer -= dt;
        if self.timer > 0.0 {
            return None;
        }
        self.timer = 0.3;
        let board = read_window(plat, game, look)?;
        if board.lost() || !board.plausible() {
            return None;
        }
        let opened = board.opened();
        if opened < self.last_opened {
            // New game.
            self.uncovers = 0;
            self.next_at = Self::gap(rng);
            self.ours = false;
        } else if opened > self.last_opened {
            if self.ours {
                self.ours = false;
            } else {
                self.uncovers += 1;
            }
        }
        self.last_opened = opened;
        // Never makes the first move, and waits for its turn.
        if opened == 0 || !ready || self.uncovers < self.next_at {
            return None;
        }
        let (cell, risky) = if mistakes && rng.chance(0.2) {
            let hidden: Vec<usize> = (0..board.cells.len()).filter(|&i| board.cells[i] == Cell::Hidden).collect();
            if hidden.is_empty() {
                return None;
            }
            let i = hidden[((rng.f32() * hidden.len() as f32) as usize).min(hidden.len() - 1)];
            ((i % board.cols, i / board.cols), true)
        } else {
            let safe = safe_cells(&board);
            if safe.is_empty() {
                // Nothing it can be sure about: don't guess.
                return None;
            }
            (safe[((rng.f32() * safe.len() as f32) as usize).min(safe.len() - 1)], false)
        };
        self.pending = Some(Pending { game, cell, risky, age: 0.0 });
        Some(board.top(cell.0, cell.1))
    }

    /// The stickman couldn't go (busy, hurt...).
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// The stickman's feet just hit the tile: press it, if it's still safe.
    pub fn stomped(&mut self, plat: &mut dyn Platform, look: Look, rng: &mut Rng) {
        let Some(p) = self.pending.take() else { return };
        let Some(board) = read_window(plat, p.game, look) else { return };
        let (c, r) = p.cell;
        if c >= board.cols || r >= board.rows || board.at(c, r) != Cell::Hidden || board.lost() {
            return;
        }
        // The board may have changed while it was jumping: re-prove it.
        if !p.risky && (!board.plausible() || !safe_cells(&board).contains(&p.cell)) {
            return;
        }
        plat.post_click(p.game, board.center(c, r));
        self.ours = true;
        self.uncovers = 0;
        self.next_at = Self::gap(rng);
    }
}

/// Debugging aid: saves what the stickman sees of the board, with its
/// reading drawn on top, to %APPDATA%\stickman\minesweeper.png.
pub fn snapshot(plat: &mut dyn Platform, game: WinId, look: Look) -> Result<PathBuf, String> {
    let scale = plat.dpi_scale();
    let (img, origin) = grab(plat, game).ok_or("couldn't capture the Minesweeper window")?;
    let board = read(&img, origin, look, scale);
    let size = IntSize::from_wh(img.w, img.h).ok_or("empty capture")?;
    let mut pm = Pixmap::from_vec(img.rgba.clone(), size).ok_or("bad capture")?;
    if let Some(b) = &board {
        let safe = safe_cells(b);
        let mut paint = Paint::default();
        let mut mark = |pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]| {
            paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
            if let Some(r) = SkRect::from_xywh(x, y, w, h) {
                pm.fill_rect(r, &paint, Transform::identity(), None);
            }
        };
        for r in 0..b.rows {
            for c in 0..b.cols {
                let p = b.center(c, r) - origin;
                let q = b.pitch.x * 0.12;
                match b.at(c, r) {
                    Cell::Hidden => mark(&mut pm, p.x - q, p.y - q, q * 2.0, q * 2.0, [0, 0, 255, 200]),
                    Cell::Flag => mark(&mut pm, p.x - q, p.y - q, q * 2.0, q * 2.0, [255, 0, 0, 220]),
                    Cell::Mine => mark(&mut pm, p.x - q * 2.0, p.y - q * 2.0, q * 4.0, q * 4.0, [255, 0, 0, 255]),
                    Cell::Open(n) => {
                        for k in 0..n {
                            mark(&mut pm, p.x - q * 3.0 + k as f32 * q, p.y + q * 2.0, q * 0.7, q * 0.7, [0, 0, 0, 255]);
                        }
                    }
                }
                if safe.contains(&(c, r)) {
                    mark(&mut pm, p.x - q * 3.0, p.y - q * 3.0, q * 6.0, q * 0.6, [0, 200, 0, 255]);
                }
            }
        }
        // Grid lines.
        for c in 0..=b.cols {
            let x = b.origin.x - origin.x + c as f32 * b.pitch.x;
            mark(&mut pm, x, b.origin.y - origin.y, 1.0, b.rows as f32 * b.pitch.y, [255, 0, 255, 160]);
        }
        for r in 0..=b.rows {
            let y = b.origin.y - origin.y + r as f32 * b.pitch.y;
            mark(&mut pm, b.origin.x - origin.x, y, b.cols as f32 * b.pitch.x, 1.0, [255, 0, 255, 160]);
        }
    }
    let dir = std::env::var_os("APPDATA").map(PathBuf::from).ok_or("no APPDATA folder")?.join("stickman");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("minesweeper.png");
    pm.save_png(&path).map_err(|e| e.to_string())?;
    if board.is_some() {
        Ok(path)
    } else {
        Err(format!("couldn't find the board (raw capture saved to {})", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(rows: &[&str]) -> Board {
        // '#' hidden, 'F' flag, digits open.
        let cells = rows
            .iter()
            .flat_map(|r| {
                r.chars().map(|ch| match ch {
                    '#' => Cell::Hidden,
                    'F' => Cell::Flag,
                    d => Cell::Open(d.to_digit(10).unwrap() as u8),
                })
            })
            .collect();
        Board { cols: rows[0].len(), rows: rows.len(), cells, origin: Vec2::ZERO, pitch: v2(16.0, 16.0) }
    }

    #[test]
    fn one_two_one_pattern() {
        // Classic 1-2-1: mines above the two 1s, the middle tile is safe.
        let b = board(&["###", "121", "000"]);
        assert_eq!(safe_cells(&b), vec![(1, 0)]);
    }

    #[test]
    fn zero_neighbours_are_safe() {
        let b = board(&["##", "#0"]);
        let mut safe = safe_cells(&b);
        safe.sort();
        assert_eq!(safe, vec![(0, 0), (0, 1), (1, 0)]);
    }

    #[test]
    fn no_guessing_when_unsure() {
        // A lone 1 next to two hidden tiles: either could be the mine.
        let b = board(&["##", "1#"]);
        // The 1 at (0,1) touches (0,0), (1,0), (1,1): no deduction possible.
        assert!(safe_cells(&b).is_empty());
    }

    #[test]
    fn wrong_flags_are_ignored() {
        // The user flagged a tile that is NOT the mine. The 1 says exactly one
        // of the two tiles is a mine; trusting the flag would wrongly free the
        // other one. We must not.
        let b = board(&["F#", "11", "00"]);
        assert!(!safe_cells(&b).contains(&(1, 0)));
    }

    #[test]
    fn implausible_board_detected() {
        let b = board(&["0#", "3#"]);
        assert!(!b.plausible());
    }

    #[test]
    fn digit_colours() {
        assert_eq!(digit([0, 0, 255]), Some(1));
        assert_eq!(digit([0, 128, 0]), Some(2));
        assert_eq!(digit([255, 0, 0]), Some(3));
        assert_eq!(digit([0, 0, 128]), Some(4));
        assert_eq!(digit([128, 0, 0]), Some(5));
        assert_eq!(digit([0, 128, 128]), Some(6));
        assert_eq!(digit([0, 0, 0]), Some(7));
        assert_eq!(digit([128, 128, 128]), Some(8));
    }
}
