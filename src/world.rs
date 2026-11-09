//! What the stickman knows about the desktop: the work area and the
//! open windows (in z-order), plus the surfaces and walls they create.

use crate::math::Rect;

pub type WinId = u64;

#[derive(Clone, Copy, Debug)]
pub struct WinInfo {
    pub id: WinId,
    pub rect: Rect,
}

/// What the stickman is standing on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// The bottom of the work area (top of the taskbar).
    Floor,
    /// The top edge of a window.
    Window(WinId),
}

/// A horizontal line you can stand on.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub support: Support,
    pub x0: f32,
    pub x1: f32,
    pub y: f32,
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

pub struct World {
    pub work: Rect,
    /// Windows ordered top-most first.
    pub windows: Vec<WinInfo>,
}

impl World {
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
            }),
            Support::Window(id) => {
                let r = self.window(id)?.rect;
                let x0 = r.l.max(self.work.l);
                let x1 = r.r.min(self.work.r);
                // Needs to be wide enough and leave headroom above it.
                if x1 - x0 < 30.0 || r.t < self.work.t + 60.0 || r.t > self.work.b - 30.0 {
                    return None;
                }
                Some(Surface { support, x0, x1, y: r.t })
            }
        }
    }

    pub fn surfaces(&self) -> Vec<Surface> {
        let mut out = Vec::with_capacity(self.windows.len() + 1);
        out.extend(self.surface(Support::Floor));
        for w in &self.windows {
            out.extend(self.surface(Support::Window(w.id)));
        }
        out
    }

    /// X coordinate of a window's side wall, if it is on screen.
    pub fn wall_x(&self, id: WinId, side: Side) -> Option<f32> {
        let r = self.window(id)?.rect;
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
    pub fn set_window_rect(&mut self, id: WinId, rect: Option<Rect>) {
        match rect {
            Some(r) => {
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                    w.rect = r;
                }
            }
            None => self.windows.retain(|w| w.id != id),
        }
    }
}
