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
}

impl Support {
    pub fn window(self) -> Option<WinId> {
        match self {
            Support::Floor => None,
            Support::Window(id) | Support::Inside(id) => Some(id),
        }
    }
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
            Support::Window(id) | Support::Inside(id) => Layer::On(id),
            Support::Floor if current == Layer::Behind => Layer::Behind,
            Support::Floor => Layer::Front,
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

    pub fn surface(&self, support: Support) -> Option<Surface> {
        match support {
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
        out
    }

    /// Window interiors only catch you if you're already in that window's
    /// layer or deliberately aiming for them; otherwise you'd get "stuck"
    /// inside every window you fall past.
    pub fn can_land(&self, s: &Surface, layer: Layer, target: Option<Support>) -> bool {
        match s.support {
            Support::Inside(id) => layer == Layer::On(id) || target == Some(s.support),
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
