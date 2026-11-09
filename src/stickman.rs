//! The stickman's body: physics and the low-level actions (walk, jump,
//! climb, mantle, get dragged). The brain picks *what* to do; this module
//! does it, and the rig turns the result into a pose.

use std::f32::consts::PI;

use crate::brain::{Intent, Perception};
use crate::math::*;
use crate::rig::{Rig, RigInput, RigState};
use crate::world::*;

/// Pixels/s² at scale 1.
pub const GRAVITY: f32 = 1900.0;
/// Highest jump the brain will attempt, at scale 1.
pub const MAX_JUMP: f32 = 230.0;

/// Body proportions in screen pixels.
#[derive(Clone, Copy, Debug)]
pub struct Dims {
    pub s: f32,
    pub thigh: f32,
    pub shin: f32,
    pub torso: f32,
    pub neck: f32,
    pub head_r: f32,
    pub upper_arm: f32,
    pub forearm: f32,
    /// Hip height above the ground when standing.
    pub stance: f32,
    pub stroke: f32,
}

impl Dims {
    pub fn new(s: f32) -> Self {
        Dims {
            s,
            thigh: 15.0 * s,
            shin: 15.0 * s,
            torso: 19.0 * s,
            neck: 2.5 * s,
            head_r: 6.5 * s,
            upper_arm: 11.5 * s,
            forearm: 11.5 * s,
            stance: 27.5 * s,
            stroke: 3.2 * s,
        }
    }

    pub fn arm(&self) -> f32 {
        self.upper_arm + self.forearm
    }

    /// How far above the hip the hands can grab.
    fn reach(&self) -> f32 {
        36.0 * self.s
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Ground(Support),
    Air,
    Climb { win: WinId, side: Side },
    /// Pulling up over a window's top edge. `from` is relative to the window.
    Mantle { win: WinId, side: Side, t: f32, from: Vec2 },
    Held,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Idle(f32),
    Walk { x: f32, speed: f32 },
    Sit(f32),
    Wave(f32),
    JumpPrep { t: f32, target: Vec2 },
    ClimbApproach { win: WinId, side: Side },
    Recover(f32),
}

pub struct Stickman {
    pub d: Dims,
    /// Hip position (when standing, exactly `stance` above the ground).
    pub pos: Vec2,
    pub vel: Vec2,
    pub mode: Mode,
    action: Option<Action>,
    pub layer: Layer,
    pub force_front: bool,
    pub facing: f32,
    time: f32,
    crouch: f32,
    /// Window we're attached to and where its corner was last frame.
    anchor: Option<(WinId, Vec2)>,
    grab_offset: Vec2,
    pending_climb: Option<(WinId, Side)>,
    pub rig: Rig,
}

fn window_origin(world: &World, id: WinId) -> Option<Vec2> {
    world.window(id).map(|w| v2(w.rect.l, w.rect.t))
}

impl Stickman {
    pub fn new(d: Dims, pos: Vec2) -> Self {
        Stickman {
            d,
            pos,
            vel: Vec2::ZERO,
            mode: Mode::Air,
            action: None,
            layer: Layer::Front,
            force_front: false,
            facing: 1.0,
            time: 0.0,
            crouch: 0.0,
            anchor: None,
            grab_offset: Vec2::ZERO,
            pending_climb: None,
            rig: Rig::new(),
        }
    }

    pub fn wants_intent(&self) -> bool {
        matches!(self.mode, Mode::Ground(_)) && self.action.is_none()
    }

    pub fn perceive(&self, cursor: Vec2) -> Option<Perception> {
        match self.mode {
            Mode::Ground(support) => Some(Perception {
                hip: self.pos,
                foot: self.pos + v2(0.0, self.d.stance),
                support,
                scale: self.d.s,
                max_jump: MAX_JUMP * self.d.s,
                cursor,
            }),
            _ => None,
        }
    }

    /// The window whose rectangle we need every frame (not just on refresh).
    pub fn attached_window(&self) -> Option<WinId> {
        match self.mode {
            Mode::Ground(Support::Window(id)) | Mode::Climb { win: id, .. } | Mode::Mantle { win: id, .. } => {
                Some(id)
            }
            _ => match self.action {
                Some(Action::ClimbApproach { win, .. }) => Some(win),
                _ => self.pending_climb.map(|(id, _)| id),
            },
        }
    }

    pub fn is_resting(&self) -> bool {
        matches!(self.mode, Mode::Ground(_))
            && matches!(self.action, Some(Action::Sit(_)) | Some(Action::Idle(_)) | None)
            && self.vel.x.abs() < 1.0
    }

    pub fn grab(&mut self, cursor: Vec2) {
        self.mode = Mode::Held;
        self.action = None;
        self.grab_offset = self.pos - cursor;
        self.layer = Layer::Front;
        self.anchor = None;
        self.pending_climb = None;
        self.vel = Vec2::ZERO;
    }

    /// Let go after a drag. Returns true if we were actually being held.
    pub fn release(&mut self) -> bool {
        if self.mode != Mode::Held {
            return false;
        }
        self.mode = Mode::Air;
        let max = 2600.0 * self.d.s;
        let l = self.vel.len();
        if l > max {
            self.vel = self.vel * (max / l);
        }
        true
    }

    pub fn teleport(&mut self, p: Vec2) {
        self.pos = p;
        self.vel = Vec2::ZERO;
        self.mode = Mode::Air;
        self.action = None;
        self.layer = Layer::Front;
        self.anchor = None;
        self.pending_climb = None;
    }

    pub fn apply(&mut self, intent: Intent) {
        if !matches!(self.mode, Mode::Ground(_)) {
            return;
        }
        self.action = Some(match intent {
            Intent::Idle { secs } => Action::Idle(secs),
            Intent::Walk { x, speed } => Action::Walk { x, speed },
            Intent::Sit { secs } => Action::Sit(secs),
            Intent::Wave { secs } => Action::Wave(secs),
            Intent::Jump { target } => {
                if (target.x - self.pos.x).abs() > 1.0 {
                    self.facing = (target.x - self.pos.x).signum();
                }
                Action::JumpPrep { t: 0.2, target }
            }
            Intent::Climb { win, side } => Action::ClimbApproach { win, side },
            Intent::ToggleLayer => {
                if !self.force_front {
                    self.layer = match self.layer {
                        Layer::Front => Layer::Behind,
                        _ => Layer::Front,
                    };
                }
                Action::Idle(0.6)
            }
        });
    }

    /// Advance one physics step. Returns the impact speed if we landed.
    pub fn update(&mut self, dt: f32, world: &World, cursor: Vec2) -> Option<f32> {
        self.time += dt;
        self.crouch = 0.0;
        let mut landed = None;
        let mode = self.mode;
        let state = match mode {
            Mode::Ground(support) => self.update_ground(dt, world, support),
            Mode::Air => {
                landed = self.update_air(dt, world);
                RigState::Air
            }
            Mode::Climb { win, side } => self.update_climb(dt, world, win, side),
            Mode::Mantle { win, side, t, from } => self.update_mantle(dt, world, win, side, t, from),
            Mode::Held => {
                let target = cursor + self.grab_offset;
                self.vel = self.vel.lerp((target - self.pos) / dt, 0.25);
                self.pos = target;
                RigState::Held
            }
        };
        if self.force_front && self.mode != Mode::Held {
            self.layer = Layer::Front;
        }
        let input = RigInput {
            hip: self.pos,
            vel: self.vel,
            facing: self.facing,
            state,
            crouch: self.crouch,
            time: self.time,
        };
        let d = self.d;
        self.rig.update(&input, &d, dt);
        landed
    }

    /// Follow a window we're attached to if it moved.
    fn carry(&mut self, world: &World, win: Option<WinId>) {
        let now = win.and_then(|id| window_origin(world, id).map(|o| (id, o)));
        if let (Some((id, o)), Some((prev_id, prev))) = (now, self.anchor) {
            if id == prev_id {
                let delta = o - prev;
                if delta != Vec2::ZERO {
                    self.pos += delta;
                    self.rig.shift(delta);
                }
            }
        }
        self.anchor = now;
    }

    fn start_fall(&mut self) {
        self.mode = Mode::Air;
        self.anchor = None;
    }

    fn update_ground(&mut self, dt: f32, world: &World, support: Support) -> RigState {
        let d = self.d;
        let s = d.s;
        let Some(surf) = world.surface(support) else {
            self.start_fall();
            return RigState::Air;
        };
        self.carry(world, match support {
            Support::Window(id) => Some(id),
            Support::Floor => None,
        });

        let mut desired = 0.0;
        let mut sit = false;
        let mut wave = false;
        if let Some(action) = self.action {
            let next = match action {
                Action::Idle(t) => (t > dt).then(|| Action::Idle(t - dt)),
                Action::Sit(t) => {
                    sit = true;
                    (t > dt).then(|| Action::Sit(t - dt))
                }
                Action::Wave(t) => {
                    wave = true;
                    (t > dt).then(|| Action::Wave(t - dt))
                }
                Action::Recover(t) => {
                    self.crouch = (t / 0.5).min(1.0) * 0.8;
                    (t > dt).then(|| Action::Recover(t - dt))
                }
                Action::Walk { x, speed } => {
                    let dx = x - self.pos.x;
                    if dx.abs() < 2.0 * s {
                        None
                    } else {
                        desired = dx.signum() * speed.min(dx.abs() * 5.0 + 20.0 * s);
                        Some(action)
                    }
                }
                Action::JumpPrep { t, target } => {
                    self.crouch = 1.0;
                    if t <= dt {
                        self.launch(target);
                        return RigState::Air;
                    }
                    Some(Action::JumpPrep { t: t - dt, target })
                }
                Action::ClimbApproach { win, side } => match world.wall_x(win, side) {
                    None => None,
                    Some(wx) => {
                        let stand = wx + side.outward() * 9.0 * s;
                        let dx = stand - self.pos.x;
                        if dx.abs() < 2.5 * s {
                            self.pos.x = stand;
                            self.vel.x = 0.0;
                            self.facing = -side.outward();
                            self.action = None;
                            if self.try_grab(world, win, side) {
                                return RigState::Air;
                            }
                            None
                        } else {
                            desired = dx.signum() * 150.0 * s;
                            Some(action)
                        }
                    }
                },
            };
            self.action = next;
        }

        let accel = if desired.abs() > self.vel.x.abs() { 1300.0 } else { 2200.0 } * s;
        self.vel.x = approach(self.vel.x, desired, accel * dt);
        self.pos.x += self.vel.x * dt;
        if desired.abs() > 1.0 {
            self.facing = desired.signum();
        }
        self.pos.y = surf.y - d.stance;
        self.vel.y = 0.0;

        let (lim0, lim1) = (world.work.l + 8.0 * s, world.work.r - 8.0 * s);
        if self.pos.x < lim0 || self.pos.x > lim1 {
            self.pos.x = self.pos.x.clamp(lim0, lim1);
            self.vel.x = 0.0;
            if matches!(self.action, Some(Action::Walk { .. })) {
                self.action = None;
            }
        }
        // Walked off the edge of a window.
        if self.pos.x < surf.x0 - 3.0 * s || self.pos.x > surf.x1 + 3.0 * s {
            self.start_fall();
        }
        RigState::Stand { ground_y: surf.y, sit, wave }
    }

    /// Ballistic jump that lands the feet on `target`.
    fn launch(&mut self, target: Vec2) {
        let g = GRAVITY * self.d.s;
        let foot = v2(self.pos.x, self.pos.y + self.d.stance);
        let apex = foot.y.min(target.y) - (35.0 * self.d.s + (target.x - foot.x).abs() * 0.12);
        let up = (foot.y - apex).max(1.0);
        let down = (target.y - apex).max(1.0);
        let flight = (2.0 * up / g).sqrt() + (2.0 * down / g).sqrt();
        self.vel = v2((target.x - foot.x) / flight, -(2.0 * g * up).sqrt());
        self.mode = Mode::Air;
        self.anchor = None;
        self.action = None;
    }

    /// Standing next to a wall: grab it, or jump up to reach it.
    fn try_grab(&mut self, world: &World, win: WinId, side: Side) -> bool {
        let Some(w) = world.window(win) else { return false };
        let r = w.rect;
        let reach = self.d.reach();
        if self.pos.y <= r.b + reach && self.pos.y >= r.t {
            self.enter_climb(world, win, side);
            return true;
        }
        let target_y = r.b + reach * 0.5;
        let h = self.pos.y - target_y;
        if h <= 0.0 || h > MAX_JUMP * self.d.s {
            return false;
        }
        self.vel = v2(0.0, -(2.0 * GRAVITY * self.d.s * h).sqrt());
        self.mode = Mode::Air;
        self.anchor = None;
        self.pending_climb = Some((win, side));
        true
    }

    fn enter_climb(&mut self, world: &World, win: WinId, side: Side) {
        self.mode = Mode::Climb { win, side };
        self.layer = Layer::On(win);
        self.anchor = window_origin(world, win).map(|o| (win, o));
        self.vel = Vec2::ZERO;
        self.pending_climb = None;
        self.action = None;
        self.facing = -side.outward();
    }

    fn update_air(&mut self, dt: f32, world: &World) -> Option<f32> {
        let d = self.d;
        let s = d.s;
        let prev_foot = self.pos.y + d.stance;
        self.vel.y += GRAVITY * s * dt;
        self.pos += self.vel * dt;

        let w = world.work;
        let (l, r) = (w.l + 8.0 * s, w.r - 8.0 * s);
        if self.pos.x < l {
            self.pos.x = l;
            self.vel.x = self.vel.x.abs() * 0.5;
        }
        if self.pos.x > r {
            self.pos.x = r;
            self.vel.x = -self.vel.x.abs() * 0.5;
        }
        let top = w.t + 45.0 * s;
        if self.pos.y < top {
            self.pos.y = top;
            self.vel.y = self.vel.y.max(0.0);
        }

        // Jumped up to catch a wall?
        if let Some((win, side)) = self.pending_climb {
            if self.vel.y > -80.0 * s {
                if let (Some(wi), Some(wx)) = (world.window(win), world.wall_x(win, side)) {
                    let r = wi.rect;
                    let stand = wx + side.outward() * 9.0 * s;
                    if (self.pos.x - stand).abs() < 16.0 * s
                        && self.pos.y <= r.b + d.reach()
                        && self.pos.y >= r.t
                    {
                        self.enter_climb(world, win, side);
                        return None;
                    }
                }
                if self.vel.y > 150.0 * s {
                    self.pending_climb = None;
                }
            }
        }

        if self.vel.y >= 0.0 {
            let foot = self.pos.y + d.stance;
            let mut best: Option<Surface> = None;
            for surf in world.surfaces() {
                if self.pos.x >= surf.x0 - 2.0
                    && self.pos.x <= surf.x1 + 2.0
                    && prev_foot <= surf.y + 1.0
                    && foot >= surf.y
                    && best.map_or(true, |b| surf.y < b.y)
                {
                    best = Some(surf);
                }
            }
            if best.is_none() && foot >= w.b {
                best = world.surface(Support::Floor);
            }
            if let Some(surf) = best {
                let impact = self.vel.y;
                self.land(surf, world);
                return Some(impact);
            }
        }
        None
    }

    fn land(&mut self, surf: Surface, world: &World) {
        let impact = self.vel.y;
        self.mode = Mode::Ground(surf.support);
        self.pos.y = surf.y - self.d.stance;
        self.vel.y = 0.0;
        self.vel.x *= 0.35;
        self.rig.on_land(impact);
        self.layer = if self.force_front {
            Layer::Front
        } else {
            match surf.support {
                Support::Window(id) => Layer::On(id),
                Support::Floor if self.layer == Layer::Behind => Layer::Behind,
                Support::Floor => Layer::Front,
            }
        };
        self.anchor = match surf.support {
            Support::Window(id) => window_origin(world, id).map(|o| (id, o)),
            Support::Floor => None,
        };
        self.pending_climb = None;
        self.action = Some(if impact > 1300.0 * self.d.s {
            Action::Recover(0.5)
        } else {
            Action::Idle(0.15)
        });
    }

    fn update_climb(&mut self, dt: f32, world: &World, win: WinId, side: Side) -> RigState {
        let s = self.d.s;
        let (Some(w), Some(wx)) = (world.window(win).copied(), world.wall_x(win, side)) else {
            self.start_fall();
            return RigState::Air;
        };
        self.carry(world, Some(win));
        let r = w.rect;
        self.pos.x = wx + side.outward() * 7.0 * s;
        self.facing = -side.outward();
        // Climb in little pulses rather than at a constant speed.
        let speed = 80.0 * s * (0.6 + 0.4 * (self.time * 8.0).sin().abs());
        self.pos.y -= speed * dt;
        self.vel = v2(0.0, -speed);
        if self.pos.y <= r.t + 4.0 * s {
            self.mode = Mode::Mantle { win, side, t: 0.0, from: self.pos - v2(r.l, r.t) };
        } else if self.pos.y > r.b + 60.0 * s {
            self.start_fall();
        }
        RigState::Climb { wall_x: wx }
    }

    fn update_mantle(&mut self, dt: f32, world: &World, win: WinId, side: Side, t: f32, from: Vec2) -> RigState {
        let d = self.d;
        let (Some(w), Some(wx)) = (world.window(win).copied(), world.wall_x(win, side)) else {
            self.start_fall();
            return RigState::Air;
        };
        let r = w.rect;
        let origin = v2(r.l, r.t);
        let t = (t + dt / 0.45).min(1.0);
        let start = origin + from;
        let end = v2(wx - side.outward() * 12.0 * d.s, r.t - d.stance);
        self.pos = start.lerp(end, smoothstep(t)) - v2(0.0, (t * PI).sin() * 6.0 * d.s);
        self.vel = Vec2::ZERO;
        if t >= 1.0 {
            self.mode = Mode::Ground(Support::Window(win));
            self.anchor = Some((win, origin));
            self.layer = Layer::On(win);
            self.action = Some(Action::Idle(0.4));
        } else {
            self.mode = Mode::Mantle { win, side, t, from };
        }
        RigState::Mantle { wall_x: wx, top_y: r.t }
    }
}
