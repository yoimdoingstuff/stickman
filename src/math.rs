//! Small 2D math helpers: vectors, rectangles and springs.

use std::f32::consts::TAU;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

pub const fn v2(x: f32, y: f32) -> Vec2 {
    Vec2 { x, y }
}

impl Vec2 {
    pub const ZERO: Vec2 = v2(0.0, 0.0);

    pub fn len(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn lerp(self, o: Vec2, t: f32) -> Vec2 {
        self + (o - self) * t
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        v2(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        v2(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f32) -> Vec2 {
        v2(self.x * k, self.y * k)
    }
}
impl Div<f32> for Vec2 {
    type Output = Vec2;
    fn div(self, k: f32) -> Vec2 {
        v2(self.x / k, self.y / k)
    }
}
impl Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        v2(-self.x, -self.y)
    }
}
impl AddAssign for Vec2 {
    fn add_assign(&mut self, o: Vec2) {
        self.x += o.x;
        self.y += o.y;
    }
}
impl SubAssign for Vec2 {
    fn sub_assign(&mut self, o: Vec2) {
        self.x -= o.x;
        self.y -= o.y;
    }
}

/// Move `cur` toward `target` by at most `max_delta`.
pub fn approach(cur: f32, target: f32, max_delta: f32) -> f32 {
    if cur < target {
        (cur + max_delta).min(target)
    } else {
        (cur - max_delta).max(target)
    }
}

pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Axis-aligned rectangle in screen pixels (y grows downward).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl Rect {
    pub fn width(&self) -> f32 {
        self.r - self.l
    }

    /// Strict overlap (touching edges don't count).
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.l < o.r && self.r > o.l && self.t < o.b && self.b > o.t
    }
}

/// Damped spring on a single value. `freq` is in Hz, `damping` 1.0 = critical.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spring {
    pub x: f32,
    pub v: f32,
}

impl Spring {
    pub fn update(&mut self, target: f32, freq: f32, damping: f32, dt: f32) {
        let w = TAU * freq;
        self.v += (w * w * (target - self.x) - 2.0 * damping * w * self.v) * dt;
        self.x += self.v * dt;
    }
}
