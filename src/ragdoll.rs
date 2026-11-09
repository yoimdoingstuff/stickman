//! Verlet ragdoll. Each joint is a particle; bones are distance constraints.
//! A few "minimum distance" constraints stop the body folding through itself.
//! Particles can be pinned (to the mouse, or by the hands to the cursor).

use crate::math::*;
use crate::rig::Pose;
use crate::stickman::Dims;
use crate::world::*;

const HEAD: usize = 0;
const CHEST: usize = 1;
const HIP: usize = 2;
const KNEE: [usize; 2] = [3, 5];
pub const FOOT: [usize; 2] = [4, 6];
const ELBOW: [usize; 2] = [7, 9];
pub const HAND: [usize; 2] = [8, 10];
const N: usize = 11;
const ITERATIONS: usize = 8;

/// (a, b, length, exact). Non-exact links only push apart (minimum distance).
fn links(d: &Dims) -> [(usize, usize, f32, bool); 17] {
    let leg = d.thigh + d.shin;
    let arm = d.arm();
    let spine = d.torso + d.neck + d.head_r;
    [
        (HEAD, CHEST, d.neck + d.head_r, true),
        (CHEST, HIP, d.torso, true),
        (HIP, KNEE[0], d.thigh, true),
        (KNEE[0], FOOT[0], d.shin, true),
        (HIP, KNEE[1], d.thigh, true),
        (KNEE[1], FOOT[1], d.shin, true),
        (CHEST, ELBOW[0], d.upper_arm, true),
        (ELBOW[0], HAND[0], d.forearm, true),
        (CHEST, ELBOW[1], d.upper_arm, true),
        (ELBOW[1], HAND[1], d.forearm, true),
        (HEAD, HIP, spine * 0.85, false),
        (HIP, FOOT[0], leg * 0.35, false),
        (HIP, FOOT[1], leg * 0.35, false),
        (CHEST, HAND[0], arm * 0.35, false),
        (CHEST, HAND[1], arm * 0.35, false),
        (HEAD, KNEE[0], d.torso * 0.9, false),
        (HEAD, KNEE[1], d.torso * 0.9, false),
    ]
}

pub struct Ragdoll {
    p: [Vec2; N],
    prev: [Vec2; N],
    /// (particle, offset from the anchor point)
    pins: Vec<(usize, Vec2)>,
    anchor: Vec2,
    still: f32,
    last_dt: f32,
    /// Window the body is trapped inside (if it went limp in one).
    pub room: Option<WinId>,
    /// Windows the body was already overlapping (not solid); worked out on
    /// the first step after it starts moving freely.
    ignore: Option<Vec<WinId>>,
    /// False while being dragged, so you can pull it anywhere.
    solid: bool,
}

impl Ragdoll {
    pub fn new(pose: &Pose, vel: Vec2, room: Option<WinId>) -> Self {
        let p = [
            pose.head,
            pose.chest,
            pose.hip,
            pose.knees[0],
            pose.feet[0],
            pose.knees[1],
            pose.feet[1],
            pose.elbows[0],
            pose.hands[0],
            pose.elbows[1],
            pose.hands[1],
        ];
        let dt = 1.0 / 120.0;
        let mut prev = p;
        for q in prev.iter_mut() {
            *q -= vel * dt;
        }
        Ragdoll {
            p,
            prev,
            pins: Vec::new(),
            anchor: Vec2::ZERO,
            still: 0.0,
            last_dt: dt,
            room,
            ignore: None,
            solid: true,
        }
    }

    /// Being dragged by the user: pass through windows. When let go, the
    /// windows it's overlapping at that moment stay non-solid.
    pub fn set_held(&mut self, held: bool) {
        self.solid = !held;
        self.ignore = None;
    }

    pub fn hip(&self) -> Vec2 {
        self.p[HIP]
    }

    pub fn velocity(&self) -> Vec2 {
        (self.p[HIP] - self.prev[HIP]) / self.last_dt
    }

    pub fn nearest(&self, pt: Vec2) -> usize {
        let mut best = (HIP, f32::MAX);
        for (i, q) in self.p.iter().enumerate() {
            let dist = (*q - pt).len();
            if dist < best.1 {
                best = (i, dist);
            }
        }
        best.0
    }

    pub fn pin(&mut self, i: usize, anchor: Vec2, offset: Vec2) {
        self.anchor = anchor;
        self.pins.retain(|(j, _)| *j != i);
        self.pins.push((i, offset));
    }

    pub fn set_anchor(&mut self, anchor: Vec2) {
        self.anchor = anchor;
    }

    pub fn clear_pins(&mut self) {
        self.pins.clear();
    }

    pub fn add_velocity(&mut self, v: Vec2) {
        for q in self.prev.iter_mut() {
            *q -= v * self.last_dt;
        }
        self.still = 0.0;
    }

    /// Nudge one particle's velocity.
    pub fn kick(&mut self, i: usize, dv: Vec2) {
        self.prev[i] -= dv * self.last_dt;
    }

    pub fn settled(&self) -> bool {
        self.still > 0.6
    }

    pub fn lowest(&self) -> f32 {
        self.p.iter().fold(f32::MIN, |m, q| m.max(q.y))
    }

    /// Move the whole body by `delta` this step (gains that as velocity).
    pub fn shove(&mut self, delta: Vec2) {
        for q in self.p.iter_mut() {
            *q += delta;
        }
        self.still = 0.0;
    }

    /// `gravity` in px/s².
    pub fn step(&mut self, dt: f32, d: &Dims, world: &World, layer: Layer, gravity: f32) {
        let g = v2(0.0, gravity) * (dt * dt);
        let start = self.p;
        for i in 0..N {
            let v = (self.p[i] - self.prev[i]) * 0.996;
            self.prev[i] = self.p[i];
            self.p[i] += v + g;
        }

        let surfaces: Vec<Surface> =
            world.surfaces().into_iter().filter(|s| world.can_land(s, layer, None)).collect();
        let room = self.room.and_then(|id| world.window(id)).map(|w| w.rect);
        let mut solids: Vec<Rect> = if self.solid {
            let pts = self.p;
            let ignore = self.ignore.get_or_insert_with(|| {
                world
                    .windows
                    .iter()
                    .filter(|w| pts.iter().any(|q| q.x > w.rect.l && q.x < w.rect.r && q.y > w.rect.t && q.y < w.rect.b))
                    .map(|w| w.id)
                    .collect()
            });
            world
                .solids(layer)
                .into_iter()
                .filter(|(id, _)| !ignore.contains(id) && Some(*id) != self.room)
                .map(|(_, r)| r)
                .collect()
        } else {
            Vec::new()
        };
        // Other *ragdolled* stickmen are solid, so ragdolls pile up on each
        // other. Standing ones aren't: flying into them knocks them over
        // instead (handled by the hit check), rather than bouncing off.
        if self.solid {
            solids.extend(world.peers.iter().filter(|p| p.ragdoll).map(|p| p.rect));
        }
        let links = links(d);
        let mut inv = [1.0f32; N];
        for &(i, _) in &self.pins {
            inv[i] = 0.0;
        }
        let mut grounded = [false; N];

        for _ in 0..ITERATIONS {
            for &(i, off) in &self.pins {
                self.p[i] = self.anchor + off;
            }
            for &(a, b, len, exact) in links.iter() {
                let delta = self.p[b] - self.p[a];
                let dist = delta.len();
                if dist < 1e-4 || (!exact && dist >= len) {
                    continue;
                }
                let w = inv[a] + inv[b];
                if w == 0.0 {
                    continue;
                }
                let corr = delta * ((dist - len) / dist / w);
                self.p[a] += corr * inv[a];
                self.p[b] -= corr * inv[b];
            }
            self.collide(&start, &surfaces, &solids, world.work, room, d, &mut grounded);
        }
        for &(i, off) in &self.pins {
            self.p[i] = self.anchor + off;
        }

        // Ground friction: bleed off sideways speed of touching particles.
        for i in 0..N {
            if grounded[i] {
                self.prev[i].x = self.p[i].x - (self.p[i].x - self.prev[i].x) * 0.7;
            }
        }

        let motion: f32 = (0..N).map(|i| (self.p[i] - self.prev[i]).len()).sum::<f32>() / N as f32;
        if motion < 0.25 * d.s && self.pins.is_empty() {
            self.still += dt;
        } else {
            self.still = 0.0;
        }
        self.last_dt = dt;
    }

    #[allow(clippy::too_many_arguments)]
    fn collide(
        &mut self,
        start: &[Vec2; N],
        surfaces: &[Surface],
        solids: &[Rect],
        work: Rect,
        room: Option<Rect>,
        d: &Dims,
        grounded: &mut [bool; N],
    ) {
        for i in 0..N {
            let r = if i == HEAD { d.head_r } else { d.stroke * 0.6 };
            let mut q = self.p[i];
            // Bounce off the sides/bottom of solid windows (pushed back out
            // the way it came in).
            for w in solids {
                let (l, t, rr, b) = (w.l - r, w.t - r, w.r + r, w.b + r);
                if !(q.x > l && q.x < rr && q.y > t && q.y < b) {
                    continue;
                }
                let s0 = start[i];
                if s0.y <= t {
                    q.y = t;
                    grounded[i] = true;
                } else if s0.y >= b {
                    q.y = b;
                } else if s0.x <= l {
                    q.x = l;
                } else if s0.x >= rr {
                    q.x = rr;
                }
            }
            let mut q = q;
            for s in surfaces {
                if q.x >= s.x0 && q.x <= s.x1 && start[i].y + r <= s.y + 1.0 && q.y + r > s.y {
                    q.y = s.y - r;
                    grounded[i] = true;
                }
            }
            if q.y + r > work.b {
                q.y = work.b - r;
                grounded[i] = true;
            }
            q.y = q.y.max(work.t + r);
            q.x = q.x.clamp(work.l + r, work.r - r);
            if let Some(rr) = room {
                q.x = q.x.clamp(rr.l + r, rr.r - r);
                let floor = (rr.b - 3.0).min(work.b);
                if q.y + r > floor {
                    q.y = floor - r;
                    grounded[i] = true;
                }
                q.y = q.y.max(rr.t + r);
            }
            self.p[i] = q;
        }
    }

    pub fn pose(&self, d: &Dims) -> Pose {
        let p = &self.p;
        let up = p[HEAD] - p[CHEST];
        let len = up.len().max(1e-3);
        Pose {
            hip: p[HIP],
            chest: p[CHEST],
            neck: p[CHEST] + up * (d.neck / len),
            head: p[HEAD],
            knees: [p[KNEE[0]], p[KNEE[1]]],
            feet: [p[FOOT[0]], p[FOOT[1]]],
            elbows: [p[ELBOW[0]], p[ELBOW[1]]],
            hands: [p[HAND[0]], p[HAND[1]]],
        }
    }
}
