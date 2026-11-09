//! What the stickman knows about the desktop: the work area and the
//! open windows (in z-order), plus the surfaces and walls they create.

use crate::math::{v2, Rect, Vec2};

pub type WinId = u64;

/// Which screen edge the taskbar is docked to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Top,
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug)]
pub struct Taskbar {
    pub rect: Rect,
    pub edge: Edge,
}

/// The space the body takes up, given where its feet are.
pub fn body_box(foot: Vec2, s: f32) -> Rect {
    Rect { l: foot.x - 5.0 * s, r: foot.x + 5.0 * s, t: foot.y - 62.0 * s, b: foot.y }
}

#[derive(Clone, Copy, Debug)]
pub struct WinInfo {
    pub id: WinId,
    pub rect: Rect,
    /// Maximized / filling the screen: treated as background (not solid,
    /// no top to stand on, no walls to climb).
    pub maximized: bool,
}

/// What the stickman is standing on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// The bottom of the work area (top of the taskbar).
    Floor,
    /// The top edge of a window.
    Window(WinId),
    /// The bottom edge, *inside* a window (walking around in it).
    Inside(WinId),
    /// On top of another stickman (from another stickman.exe).
    Peer(u32),
    /// The top of a box on a web page (text, image, button...) inside a
    /// browser window. `key` identifies the box across page refreshes.
    Ledge { win: WinId, key: u32 },
    /// The top of a desktop icon (key from its name, so it survives you
    /// moving the icon around).
    Icon(u32),
}

/// A stable key for a desktop icon, from its name.
pub fn icon_key(name: &str) -> u32 {
    // FNV-1a.
    name.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

impl Support {
    pub fn window(self) -> Option<WinId> {
        match self {
            Support::Floor | Support::Peer(_) | Support::Ledge { .. } | Support::Icon(_) => None,
            Support::Window(id) | Support::Inside(id) => Some(id),
        }
    }

    /// The window this support is part of (including page ledges).
    pub fn host(self) -> Option<WinId> {
        match self {
            Support::Ledge { win, .. } => Some(win),
            s => s.window(),
        }
    }
}

/// A web page shown in a browser window, and the ledges on it.
#[derive(Clone, Debug)]
pub struct Page {
    pub win: WinId,
    pub doc: Rect,
    pub ledges: Vec<Rect>,
}

/// Same-shaped boxes keep their key when the page scrolls (only y changes).
pub fn ledge_key(r: &Rect) -> u32 {
    let l = (r.l / 4.0).round() as i32 as u32;
    let w = ((r.r - r.l) / 4.0).round() as i32 as u32;
    l.wrapping_mul(73_856_093) ^ w.wrapping_mul(19_349_663)
}

/// Turn a page's boxes into ledges worth standing on: wide enough, not the
/// giant containers, with headroom, and with near-duplicates (a paragraph and
/// the text inside it) merged.
pub fn page_ledges(doc: Rect, boxes: &[(Rect, i32)], s: f32) -> Vec<Rect> {
    // Text, image, button, hyperlink, edit, list item, group, header,
    // menu item, combo box, check box, table, data item.
    const KINDS: [i32; 13] = [50020, 50006, 50000, 50005, 50004, 50007, 50026, 50034, 50011, 50003, 50002, 50036, 50029];
    let doc_area = (doc.r - doc.l) * (doc.b - doc.t);
    let mut ledges: Vec<Rect> = boxes
        .iter()
        .filter(|(_, k)| KINDS.contains(k))
        .map(|(r, _)| Rect { l: r.l.max(doc.l), t: r.t, r: r.r.min(doc.r), b: r.b.min(doc.b) })
        .filter(|r| {
            let (w, h) = (r.r - r.l, r.b - r.t);
            w >= 40.0 * s
                && h >= 10.0 * s
                && r.t > doc.t + 50.0 * s
                && r.t < doc.b - 8.0 * s
                && w * h < doc_area * 0.4
        })
        .collect();
    ledges.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<Rect> = Vec::new();
    for r in ledges {
        let dup = out.iter_mut().find(|o| {
            let overlap = r.r.min(o.r) - r.l.max(o.l);
            (r.t - o.t).abs() < 5.0 * s && overlap > 0.6 * (r.r - r.l).min(o.r - o.l)
        });
        match dup {
            Some(o) => {
                o.l = o.l.min(r.l);
                o.r = o.r.max(r.r);
            }
            None => out.push(r),
        }
    }
    // Plenty, but not thousands.
    out.sort_by(|a, b| (b.r - b.l).partial_cmp(&(a.r - a.l)).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(250);
    out
}

/// Another stickman's body, for standing on and bumping into.
#[derive(Clone, Copy, Debug)]
pub struct PeerBody {
    pub id: u32,
    /// Bounding box of its body.
    pub rect: Rect,
    pub ragdoll: bool,
}

/// A horizontal line you can stand on.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub support: Support,
    pub x0: f32,
    pub x1: f32,
    pub y: f32,
    /// Ends are walls (inside a window) rather than ledges you fall off.
    pub walled: bool,
}

/// Which side wall of a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// Direction pointing away from the window, out of this wall.
    pub fn outward(self) -> f32 {
        match self {
            Side::Left => -1.0,
            Side::Right => 1.0,
        }
    }
}

/// Where the stickman is drawn in the window stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Above every window.
    Front,
    /// Below every window (on the desktop itself).
    Behind,
    /// Right above this window: windows higher in the stack cover it.
    On(WinId),
}

impl Layer {
    /// The layer you're on when standing on `support`.
    pub fn for_support(support: Support, current: Layer) -> Layer {
        match support {
            Support::Window(id) | Support::Inside(id) | Support::Ledge { win: id, .. } => Layer::On(id),
            Support::Peer(_) => current,
            // Icons live on the desktop itself.
            Support::Floor | Support::Icon(_) if current == Layer::Behind => Layer::Behind,
            Support::Floor | Support::Icon(_) => Layer::Front,
        }
    }
}

pub struct World {
    pub work: Rect,
    /// Windows ordered top-most first.
    pub windows: Vec<WinInfo>,
    /// Visible (not auto-hidden) taskbar, if any.
    pub taskbar: Option<Taskbar>,
    /// Windows that have a matching app button on the taskbar, and where
    /// that button is: the only places it will dive into the taskbar.
    pub doors: Vec<(WinId, Rect)>,
    /// Other stickmen's bodies.
    pub peers: Vec<PeerBody>,
    /// Web pages in browser windows it's near or in.
    pub pages: Vec<Page>,
    /// Desktop icons: (key, rect). Empty if that's turned off in settings.
    pub icons: Vec<(u32, Rect)>,
}

/// Does a taskbar button belong to this window? Button names are either the
/// window title (ungrouped) or the app name, often with " - 2 running
/// windows" tacked on. Returns a match score (0 = no).
pub fn button_matches(button: &str, title: &str, exe: &str) -> u32 {
    let b = button.to_lowercase();
    // Drop "- N running window(s)" style suffixes.
    let b: String = b.split(" - ").filter(|part| !part.contains("running window")).collect::<Vec<_>>().join(" - ");
    let b = b.trim();
    let t = title.to_lowercase();
    let t = t.trim();
    if b.is_empty() {
        return 0;
    }
    if !t.is_empty() && b == t {
        return 4;
    }
    // Title "Page - Google Chrome", button "Google Chrome".
    if !t.is_empty() && (t.ends_with(b) || t.starts_with(b)) && b.len() >= 3 {
        return 3;
    }
    if !t.is_empty() && (b.contains(t) || t.contains(b)) && b.len().min(t.len()) >= 4 {
        return 2;
    }
    // Exe "chrome.exe" vs button "Google Chrome".
    let stem = exe.trim_end_matches(".exe");
    if stem.len() >= 3 && b.replace(' ', "").contains(stem) {
        return 1;
    }
    0
}

impl World {
    /// Windows that are solid to a flying body on `layer`: the ones it's
    /// drawn in front of. Windows in front of it just cover it (it passes
    /// behind them), and on the desktop layer nothing is solid.
    pub fn solids(&self, layer: Layer) -> Vec<(WinId, Rect)> {
        let from = match layer {
            Layer::Behind => return Vec::new(),
            Layer::Front => 0,
            Layer::On(id) => self.windows.iter().position(|w| w.id == id).unwrap_or(0),
        };
        self.windows[from..].iter().filter(|w| !w.maximized).map(|w| (w.id, w.rect)).collect()
    }

    /// The taskbar button that leads to this window, if it has one.
    pub fn door(&self, id: WinId) -> Option<Rect> {
        self.doors.iter().find(|(w, _)| *w == id).map(|(_, r)| *r)
    }

    /// Windows the body is already overlapping (ignored for collisions, or
    /// it would get stuck the moment it jumps).
    pub fn overlapping(&self, foot: Vec2, s: f32) -> Vec<WinId> {
        let b = body_box(foot, s);
        self.windows.iter().filter(|w| w.rect.overlaps(&b)).map(|w| w.id).collect()
    }

    /// Would a body flying this arc clip a solid window on the way?
    #[allow(clippy::too_many_arguments)]
    pub fn arc_clear(&self, layer: Layer, foot: Vec2, vel: Vec2, g: f32, flight: f32, s: f32, skip: &[WinId]) -> bool {
        let solids: Vec<Rect> =
            self.solids(layer).into_iter().filter(|(id, _)| !skip.contains(id)).map(|(_, r)| r).collect();
        if solids.is_empty() {
            return true;
        }
        let n = 24;
        (1..n).all(|i| {
            let t = flight * i as f32 / n as f32;
            let p = foot + vel * t + v2(0.0, 0.5 * g * t * t);
            let b = body_box(p, s);
            !solids.iter().any(|r| r.overlaps(&b))
        })
    }

    /// Can the stickman dive into the taskbar from the floor?
    pub fn portal_edge(&self) -> Option<Edge> {
        self.taskbar.map(|t| t.edge).filter(|e| *e != Edge::Top)
    }
    pub fn window(&self, id: WinId) -> Option<&WinInfo> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// The page ledge with this key, the one nearest to `near_y` if several
    /// boxes share a shape (list items, etc.).
    fn ledge(&self, win: WinId, key: u32, near_y: Option<f32>) -> Option<Rect> {
        let page = self.pages.iter().find(|p| p.win == win)?;
        let mut best: Option<Rect> = None;
        for r in page.ledges.iter().filter(|r| ledge_key(r) == key) {
            let better = match (best, near_y) {
                (None, _) => true,
                (Some(b), Some(y)) => (r.t - y).abs() < (b.t - y).abs(),
                (Some(_), None) => false,
            };
            if better {
                best = Some(*r);
            }
        }
        best
    }

    /// Like `surface`, but for page ledges picks the one nearest `near_y`.
    pub fn surface_near(&self, support: Support, near_y: f32) -> Option<Surface> {
        match support {
            Support::Ledge { win, key } => {
                let r = self.ledge(win, key, Some(near_y))?;
                Some(Surface { support, x0: r.l, x1: r.r, y: r.t, walled: false })
            }
            _ => self.surface(support),
        }
    }

    pub fn icon(&self, key: u32) -> Option<Rect> {
        self.icons.iter().find(|(k, _)| *k == key).map(|(_, r)| *r)
    }

    pub fn surface(&self, support: Support) -> Option<Surface> {
        match support {
            Support::Ledge { win, key } => {
                let r = self.ledge(win, key, None)?;
                Some(Surface { support, x0: r.l, x1: r.r, y: r.t, walled: false })
            }
            Support::Icon(key) => {
                // The icon picture's top edge (a little in from the label's sides).
                let r = self.icon(key)?;
                let inset = ((r.r - r.l) * 0.15).min(12.0);
                if r.t < self.work.t + 60.0 {
                    return None;
                }
                Some(Surface { support, x0: r.l + inset, x1: r.r - inset, y: r.t + 2.0, walled: false })
            }
            Support::Floor => Some(Surface {
                support,
                x0: self.work.l,
                x1: self.work.r,
                y: self.work.b,
                walled: false,
            }),
            Support::Window(id) => {
                let w = self.window(id)?;
                if w.maximized {
                    return None;
                }
                let r = w.rect;
                let x0 = r.l.max(self.work.l);
                let x1 = r.r.min(self.work.r);
                // Needs to be wide enough and leave headroom above it.
                if x1 - x0 < 30.0 || r.t < self.work.t + 60.0 || r.t > self.work.b - 30.0 {
                    return None;
                }
                Some(Surface { support, x0, x1, y: r.t, walled: false })
            }
            Support::Peer(id) => {
                // Standing on someone's head (or on top of a ragdoll pile).
                let p = self.peers.iter().find(|p| p.id == id)?;
                let r = p.rect;
                let (x0, x1) = if p.ragdoll {
                    (r.l, r.r)
                } else {
                    let c = (r.l + r.r) * 0.5;
                    (c - 6.0, c + 6.0)
                };
                if r.t < self.work.t + 60.0 {
                    return None;
                }
                Some(Surface { support, x0, x1, y: r.t, walled: false })
            }
            Support::Inside(id) => {
                let r = self.window(id)?.rect;
                let x0 = r.l.max(self.work.l) + 6.0;
                let x1 = r.r.min(self.work.r) - 6.0;
                let y = (r.b - 3.0).min(self.work.b);
                // Big enough to be a room, and the floor is on screen.
                if x1 - x0 < 120.0 || y - r.t.max(self.work.t) < 140.0 {
                    return None;
                }
                Some(Surface { support, x0, x1, y, walled: true })
            }
        }
    }

    pub fn surfaces(&self) -> Vec<Surface> {
        let mut out = Vec::with_capacity(self.windows.len() * 2 + 1);
        out.extend(self.surface(Support::Floor));
        for w in &self.windows {
            out.extend(self.surface(Support::Window(w.id)));
            out.extend(self.surface(Support::Inside(w.id)));
        }
        for p in &self.peers {
            out.extend(self.surface(Support::Peer(p.id)));
        }
        for page in &self.pages {
            for r in &page.ledges {
                let support = Support::Ledge { win: page.win, key: ledge_key(r) };
                out.push(Surface { support, x0: r.l, x1: r.r, y: r.t, walled: false });
            }
        }
        for (key, _) in &self.icons {
            out.extend(self.surface(Support::Icon(*key)));
        }
        out
    }

    /// The point a support moves with (so we can ride along when it moves).
    /// `near` picks between same-shaped page ledges.
    pub fn support_origin(&self, support: Support, near: Option<Vec2>) -> Option<Vec2> {
        match support {
            Support::Ledge { win, key } => self.ledge(win, key, near.map(|p| p.y)).map(|r| v2(r.l, r.t)),
            Support::Icon(key) => self.icon(key).map(|r| v2(r.l, r.t)),
            Support::Floor => None,
            Support::Window(id) | Support::Inside(id) => self.window(id).map(|w| v2(w.rect.l, w.rect.t)),
            Support::Peer(id) => self.peers.iter().find(|p| p.id == id).map(|p| v2((p.rect.l + p.rect.r) * 0.5, p.rect.t)),
        }
    }

    /// Window interiors only catch you if you're already in that window's
    /// layer or deliberately aiming for them; otherwise you'd get "stuck"
    /// inside every window you fall past.
    pub fn can_land(&self, s: &Surface, layer: Layer, target: Option<Support>) -> bool {
        match s.support {
            // Inside a window (its floor, or boxes on its web page): only if
            // we're in that window already, or jumping there on purpose.
            Support::Inside(id) | Support::Ledge { win: id, .. } => layer == Layer::On(id) || target == Some(s.support),
            // Icons are on the desktop: not while inside a window.
            Support::Icon(_) => !matches!(layer, Layer::On(_)) || target == Some(s.support),
            _ => true,
        }
    }

    /// Top-most window containing a point.
    pub fn window_at(&self, p: Vec2) -> Option<WinId> {
        self.windows
            .iter()
            .find(|w| p.x >= w.rect.l && p.x <= w.rect.r && p.y >= w.rect.t && p.y <= w.rect.b)
            .map(|w| w.id)
    }

    /// X coordinate of a window's side wall, if it is on screen.
    pub fn wall_x(&self, id: WinId, side: Side) -> Option<f32> {
        let w = self.window(id)?;
        if w.maximized {
            return None;
        }
        let r = w.rect;
        let x = match side {
            Side::Left => r.l,
            Side::Right => r.r,
        };
        if x < self.work.l + 25.0 || x > self.work.r - 25.0 {
            None
        } else {
            Some(x)
        }
    }

    /// Rectangles that should hide the stickman when it is on `layer`.
    pub fn occluders(&self, layer: Layer) -> Vec<Rect> {
        match layer {
            Layer::Front => Vec::new(),
            Layer::Behind => self.windows.iter().map(|w| w.rect).collect(),
            Layer::On(id) => match self.windows.iter().position(|w| w.id == id) {
                Some(i) => self.windows[..i].iter().map(|w| w.rect).collect(),
                None => Vec::new(),
            },
        }
    }

    /// Update (or remove, if it vanished) one window between full refreshes.
    /// Returns its previous rectangle.
    pub fn set_window_rect(&mut self, id: WinId, rect: Option<Rect>) -> Option<Rect> {
        let old = self.window(id).map(|w| w.rect);
        match rect {
            Some(r) => {
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                    w.rect = r;
                }
            }
            None => self.windows.retain(|w| w.id != id),
        }
        old
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(l: f32, t: f32, r: f32, b: f32) -> Rect {
        Rect { l, t, r, b }
    }

    #[test]
    fn page_ledges_keep_good_boxes_and_merge_duplicates() {
        let doc = r(0.0, 100.0, 1000.0, 900.0);
        let boxes = vec![
            // A paragraph (group) and the text inside it: one ledge.
            (r(100.0, 300.0, 600.0, 360.0), 50026),
            (r(102.0, 301.0, 590.0, 330.0), 50020),
            // An image further down.
            (r(200.0, 500.0, 400.0, 650.0), 50006),
            // Too small, too close to the top, a giant container, and a type
            // we don't stand on (scroll bar).
            (r(50.0, 400.0, 70.0, 410.0), 50020),
            (r(100.0, 120.0, 600.0, 160.0), 50020),
            (r(0.0, 200.0, 1000.0, 900.0), 50026),
            (r(980.0, 300.0, 1000.0, 800.0), 50014),
        ];
        let ledges = page_ledges(doc, &boxes, 1.0);
        assert_eq!(ledges.len(), 2, "{ledges:?}");
        assert!(ledges.iter().any(|l| l.t == 300.0 && l.l == 100.0 && l.r == 600.0));
        assert!(ledges.iter().any(|l| l.t == 500.0));
    }

    #[test]
    fn ledge_keys_survive_scrolling() {
        // Scrolling only moves boxes up and down.
        let a = r(100.0, 300.0, 600.0, 360.0);
        let b = r(100.0, 180.0, 600.0, 240.0);
        assert_eq!(ledge_key(&a), ledge_key(&b));
        assert_ne!(ledge_key(&a), ledge_key(&r(140.0, 300.0, 600.0, 360.0)));
    }

    #[test]
    fn ledges_only_catch_you_inside_their_window() {
        let world = World {
            work: r(0.0, 0.0, 1920.0, 1040.0),
            windows: vec![WinInfo { id: 7, rect: r(100.0, 100.0, 1100.0, 900.0), maximized: false }],
            taskbar: None,
            doors: Vec::new(),
            peers: Vec::new(),
            pages: vec![Page { win: 7, doc: r(100.0, 180.0, 1100.0, 900.0), ledges: vec![r(200.0, 400.0, 500.0, 440.0)] }],
            icons: vec![(icon_key("Recycle Bin"), r(20.0, 200.0, 95.0, 270.0))],
        };
        // Icons are walkable from the desktop, not from inside the browser.
        let icon = world.surfaces().into_iter().find(|s| matches!(s.support, Support::Icon(_))).expect("icon");
        assert!(world.can_land(&icon, Layer::Front, None));
        assert!(!world.can_land(&icon, Layer::On(7), None));
        let ledge = world.surfaces().into_iter().find(|s| matches!(s.support, Support::Ledge { .. })).expect("ledge");
        assert!(world.can_land(&ledge, Layer::On(7), None));
        assert!(!world.can_land(&ledge, Layer::Front, None));
        assert!(world.surface_near(ledge.support, 410.0).is_some());
    }
}
