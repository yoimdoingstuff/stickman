//! The stickman's body: physics and the low-level actions (walk, jump, flip,
//! climb, mantle, punch, hang off the cursor, go ragdoll). The brain picks
//! *what* to do; this module does it, and the rig turns it into a pose.

use std::f32::consts::{PI, TAU};

use crate::brain::{Intent, PeerView, Perception};
use crate::math::*;
use crate::ragdoll::{Ragdoll, FOOT, HAND};
use crate::rig::{Gesture, Pose, Rig, RigInput, RigState};
use crate::world::*;

/// Pixels/s² at scale 1.
pub const GRAVITY: f32 = 1900.0;
/// Highest jump the brain will attempt, at scale 1.
pub const MAX_JUMP: f32 = 230.0;
/// Below this speed a released/dropped body lands on its feet instead of flopping.
const GENTLE: f32 = 700.0;

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
    pub fn reach(&self) -> f32 {
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
    /// Limp physics body (thrown, knocked over, picked up, hanging on the cursor).
    Ragdoll,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Idle(f32),
    Walk { x: f32, speed: f32 },
    Sit(f32),
    Wave(f32),
    Dance(f32),
    JumpPrep { t: f32, kind: JumpKind },
    ClimbApproach { win: WinId, side: Side },
    Recover(f32),
    Punch { t: f32, target: u32, dir: f32, fired: bool },
}

#[derive(Clone, Copy, Debug)]
enum JumpKind {
    To { target: Vec2, support: Support },
    Flip,
    Cursor { secs: f32 },
}

/// Things that happened during an update that the app needs to know about.
#[derive(Default)]
pub struct Events {
    pub landed: Option<f32>,
    /// (peer id, direction) when a punch connects.
    pub punch: Option<(u32, f32)>,
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
    pub allow_cursor: bool,
    pub facing: f32,
    time: f32,
    crouch: f32,
    /// Window we're attached to and where its corner was last frame.
    anchor: Option<(WinId, Vec2)>,
    pending_climb: Option<(WinId, Side)>,
    /// Surface a jump is aimed at (lets us land inside windows on purpose).
    jump_target: Option<Support>,
    /// Mid-jump toward the cursor, hoping to grab it (seconds to hang).
    chasing_cursor: Option<f32>,
    spin: f32,
    spin_rate: f32,
    ragdoll: Option<Ragdoll>,
    /// Ragdoll particle the user is dragging.
    user_pin: Option<usize>,
    /// Seconds left hanging off the cursor.
    hang: f32,
    cursor: Vec2,
    cursor_vel: Vec2,
    pub rig: Rig,
}

fn window_origin(world: &World, id: WinId) -> Option<Vec2> {
    world.window(id).map(|w| v2(w.rect.l, w.rect.t))
}

fn rotate(p: Vec2, pivot: Vec2, a: f32) -> Vec2 {
    let (s, c) = a.sin_cos();
    let d = p - pivot;
    pivot + v2(d.x * c - d.y * s, d.x * s + d.y * c)
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
            allow_cursor: true,
            facing: 1.0,
            time: 0.0,
            crouch: 0.0,
            anchor: None,
            pending_climb: None,
            jump_target: None,
            chasing_cursor: None,
            spin: 0.0,
            spin_rate: 0.0,
            ragdoll: None,
            user_pin: None,
            hang: 0.0,
            cursor: Vec2::ZERO,
            cursor_vel: Vec2::ZERO,
            rig: Rig::new(),
        }
    }

    // ---------- queries ----------

    pub fn wants_intent(&self) -> bool {
        matches!(self.mode, Mode::Ground(_)) && self.action.is_none()
    }

    pub fn perceive(&self, cursor: Vec2, peers: Vec<PeerView>) -> Option<Perception> {
        match self.mode {
            Mode::Ground(support) => Some(Perception {
                hip: self.pos,
                foot: self.pos + v2(0.0, self.d.stance),
                support,
                scale: self.d.s,
                max_jump: MAX_JUMP * self.d.s,
                reach: self.d.reach() + self.d.stance,
                cursor,
                allow_cursor: self.allow_cursor,
                peers,
            }),
            _ => None,
        }
    }

    /// The window whose rectangle we need every frame (not just on refresh).
    pub fn attached_window(&self) -> Option<WinId> {
        match self.mode {
            Mode::Ground(s) => s.window(),
            Mode::Climb { win, .. } | Mode::Mantle { win, .. } => Some(win),
            Mode::Ragdoll => self.ragdoll.as_ref().and_then(|r| r.room),
            Mode::Air => match self.action {
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

    pub fn is_ragdoll(&self) -> bool {
        self.mode == Mode::Ragdoll
    }

    /// Where the body is, for positioning the overlay window.
    pub fn center(&self) -> Vec2 {
        match &self.ragdoll {
            Some(r) => r.hip(),
            None => self.pos,
        }
    }

    pub fn pose(&self) -> Pose {
        if let Some(r) = &self.ragdoll {
            return r.pose(&self.d);
        }
        let mut p = self.rig.pose;
        if self.spin != 0.0 {
            let pivot = (p.hip + p.chest) * 0.5;
            let a = self.spin;
            let rot = |v: Vec2| rotate(v, pivot, a);
            p = Pose {
                hip: rot(p.hip),
                chest: rot(p.chest),
                neck: rot(p.neck),
                head: rot(p.head),
                knees: [rot(p.knees[0]), rot(p.knees[1])],
                feet: [rot(p.feet[0]), rot(p.feet[1])],
                elbows: [rot(p.elbows[0]), rot(p.elbows[1])],
                hands: [rot(p.hands[0]), rot(p.hands[1])],
            };
        }
        p
    }

    // ---------- outside influences ----------

    pub fn grab(&mut self, cursor: Vec2) {
        if self.ragdoll.is_none() {
            self.start_ragdoll(self.vel);
        }
        self.hang = 0.0;
        self.layer = Layer::Front;
        if let Some(r) = &mut self.ragdoll {
            r.clear_pins();
            r.room = None;
            let i = r.nearest(cursor);
            r.pin(i, cursor, Vec2::ZERO);
            self.user_pin = Some(i);
        }
    }

    /// Let go after a drag. Returns true if we were actually being held.
    pub fn release(&mut self, world: &World) -> bool {
        if self.user_pin.take().is_none() {
            return false;
        }
        let gentle = match &mut self.ragdoll {
            Some(r) => {
                r.clear_pins();
                let v = r.velocity();
                let max = 2600.0 * self.d.s;
                if v.len() > max {
                    r.add_velocity(v * (max / v.len() - 1.0));
                }
                v.len() < GENTLE * self.d.s
            }
            None => false,
        };
        if gentle {
            self.air_from_ragdoll();
            // Dropped onto a window? Then it lives inside that window.
            if let Some(id) = world.window_at(self.pos) {
                if world.surface(Support::Inside(id)).is_some() {
                    self.layer = Layer::On(id);
                }
            }
        }
        true
    }

    pub fn go_limp(&mut self) {
        let v = self.vel + v2(self.facing * 80.0, -150.0) * self.d.s;
        if self.ragdoll.is_none() {
            self.start_ragdoll(v);
        }
    }

    /// Hit by someone else.
    pub fn knock(&mut self, impulse: Vec2) {
        self.hang = 0.0;
        match &mut self.ragdoll {
            Some(r) => {
                if self.user_pin.is_none() {
                    r.clear_pins();
                }
                r.add_velocity(impulse);
            }
            None => self.start_ragdoll(self.vel + impulse),
        }
    }

    pub fn teleport(&mut self, p: Vec2) {
        self.ragdoll = None;
        self.user_pin = None;
        self.hang = 0.0;
        self.pos = p;
        self.vel = Vec2::ZERO;
        self.mode = Mode::Air;
        self.action = None;
        self.layer = Layer::Front;
        self.reset_air_state();
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
            Intent::Dance { secs } => Action::Dance(secs),
            Intent::Jump { target, support } => {
                if (target.x - self.pos.x).abs() > 1.0 {
                    self.facing = (target.x - self.pos.x).signum();
                }
                Action::JumpPrep { t: 0.2, kind: JumpKind::To { target, support } }
            }
            Intent::Flip => Action::JumpPrep { t: 0.25, kind: JumpKind::Flip },
            Intent::GrabCursor { secs } => {
                if (self.cursor.x - self.pos.x).abs() > 1.0 {
                    self.facing = (self.cursor.x - self.pos.x).signum();
                }
                Action::JumpPrep { t: 0.18, kind: JumpKind::Cursor { secs } }
            }
            Intent::Climb { win, side } => Action::ClimbApproach { win, side },
            Intent::Punch { target, dir } => {
                self.facing = dir;
                Action::Punch { t: 0.0, target, dir, fired: false }
            }
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

    // ---------- simulation ----------

    /// Advance one physics step.
    pub fn update(&mut self, dt: f32, world: &World, cursor: Vec2, cursor_vel: Vec2) -> Events {
        self.time += dt;
        self.crouch = 0.0;
        self.cursor = cursor;
        self.cursor_vel = cursor_vel;
        let mut ev = Events::default();
        let mode = self.mode;
        let state = match mode {
            Mode::Ground(support) => self.update_ground(dt, world, support, &mut ev),
            Mode::Air => {
                ev.landed = self.update_air(dt, world);
                RigState::Air
            }
            Mode::Climb { win, side } => self.update_climb(dt, world, win, side),
            Mode::Mantle { win, side, t, from } => self.update_mantle(dt, world, win, side, t, from),
            Mode::Ragdoll => {
                self.update_ragdoll(dt, world);
                if self.force_front && self.user_pin.is_none() {
                    self.layer = Layer::Front;
                }
                return ev;
            }
        };
        if self.force_front {
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
        ev
    }

    fn reset_air_state(&mut self) {
        self.anchor = None;
        self.pending_climb = None;
        self.jump_target = None;
        self.chasing_cursor = None;
        self.spin = 0.0;
        self.spin_rate = 0.0;
    }

    fn start_ragdoll(&mut self, vel: Vec2) {
        let room = match self.mode {
            Mode::Ground(Support::Inside(id)) => Some(id),
            _ => None,
        };
        let pose = self.pose();
        self.ragdoll = Some(Ragdoll::new(&pose, vel, room));
        self.mode = Mode::Ragdoll;
        self.action = None;
        self.reset_air_state();
    }

    /// Ragdoll -> controlled fall (lands on its feet).
    fn air_from_ragdoll(&mut self) {
        if let Some(r) = self.ragdoll.take() {
            let pose = r.pose(&self.d);
            self.pos = r.hip();
            self.vel = r.velocity();
            self.rig.adopt(&pose);
        }
        self.mode = Mode::Air;
        self.reset_air_state();
    }

    /// Ragdoll has come to rest: get back up on whatever it's lying on.
    fn stand_up(&mut self, world: &World) {
        let Some(r) = self.ragdoll.take() else { return };
        let pose = r.pose(&self.d);
        let hip = r.hip();
        let lowest = r.lowest();
        let room = r.room;
        let mut best: Option<Surface> = None;
        for s in world.surfaces() {
            let allowed = world.can_land(&s, self.layer, None) || room.map(Support::Inside) == Some(s.support);
            if allowed
                && hip.x >= s.x0 - 4.0
                && hip.x <= s.x1 + 4.0
                && s.y >= lowest - 4.0
                && best.map_or(true, |b| s.y < b.y)
            {
                best = Some(s);
            }
        }
        let Some(surf) = best.or_else(|| world.surface(Support::Floor)) else { return };
        self.mode = Mode::Ground(surf.support);
        self.pos = v2(hip.x.clamp(surf.x0 + 2.0, surf.x1 - 2.0), surf.y - self.d.stance);
        self.vel = Vec2::ZERO;
        if !self.force_front {
            self.layer = Layer::for_support(surf.support, self.layer);
        }
        self.reset_air_state();
        self.anchor = surf.support.window().and_then(|id| window_origin(world, id).map(|o| (id, o)));
        self.rig.adopt(&pose);
        self.action = Some(Action::Recover(0.9));
    }

    fn update_ragdoll(&mut self, dt: f32, world: &World) {
        let d = self.d;
        let s = d.s;
        let held = self.user_pin.is_some();
        let hanging = self.hang > 0.0;
        let layer = self.layer;
        let time = self.time;
        let cursor = self.cursor;
        let (vel, settled) = match &mut self.ragdoll {
            None => {
                self.mode = Mode::Air;
                return;
            }
            Some(r) => {
                if held || hanging {
                    r.set_anchor(cursor);
                }
                if hanging {
                    // Little kicks so it doesn't hang there like a wet towel.
                    for (i, &f) in FOOT.iter().enumerate() {
                        let k = (time * 3.0 + i as f32 * 1.7).sin() * 900.0 * s * dt;
                        r.kick(f, v2(k, 0.0));
                    }
                }
                r.step(dt, &d, world, layer);
                self.pos = r.hip();
                (r.velocity(), r.settled())
            }
        };
        self.vel = vel;

        if hanging {
            self.hang -= dt;
            let flung = self.cursor_vel.len() > 4500.0 * s;
            if self.hang <= 0.0 || flung || !self.allow_cursor {
                self.hang = 0.0;
                if let Some(r) = &mut self.ragdoll {
                    r.clear_pins();
                }
                if vel.len() < GENTLE * s {
                    self.air_from_ragdoll();
                }
            }
            return;
        }
        if !held && settled {
            self.stand_up(world);
        }
    }

    fn start_hang(&mut self, secs: f32) {
        self.start_ragdoll(self.vel);
        let s = self.d.s;
        if let Some(r) = &mut self.ragdoll {
            r.pin(HAND[0], self.cursor, v2(-2.0 * s, 0.0));
            r.pin(HAND[1], self.cursor, v2(2.0 * s, 0.0));
        }
        self.hang = secs;
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
        self.reset_air_state();
    }

    fn update_ground(&mut self, dt: f32, world: &World, support: Support, ev: &mut Events) -> RigState {
        let d = self.d;
        let s = d.s;
        let Some(surf) = world.surface(support) else {
            self.start_fall();
            return RigState::Air;
        };
        self.carry(world, support.window());
        if let (Support::Inside(id), false) = (support, self.force_front) {
            self.layer = Layer::On(id);
        }

        let mut desired = 0.0;
        let mut sit = false;
        let mut gesture = Gesture::None;
        if let Some(action) = self.action {
            let next = match action {
                Action::Idle(t) => (t > dt).then(|| Action::Idle(t - dt)),
                Action::Sit(t) => {
                    sit = true;
                    (t > dt).then(|| Action::Sit(t - dt))
                }
                Action::Wave(t) => {
                    gesture = Gesture::Wave;
                    (t > dt).then(|| Action::Wave(t - dt))
                }
                Action::Dance(t) => {
                    gesture = Gesture::Dance;
                    (t > dt).then(|| Action::Dance(t - dt))
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
                Action::Punch { t, target, dir, fired } => {
                    let t = t + dt;
                    let ext = if t < 0.14 {
                        -(t / 0.14)
                    } else if t < 0.24 {
                        1.0
                    } else {
                        (1.0 - (t - 0.24) / 0.16).max(0.0)
                    };
                    gesture = Gesture::Punch(ext);
                    self.facing = dir;
                    let mut fired = fired;
                    if t >= 0.14 && !fired {
                        fired = true;
                        ev.punch = Some((target, dir));
                    }
                    if (0.14..0.24).contains(&t) {
                        desired = dir * 60.0 * s;
                    }
                    (t < 0.45).then_some(Action::Punch { t, target, dir, fired })
                }
                Action::JumpPrep { t, kind } => {
                    self.crouch = 1.0;
                    if t <= dt {
                        self.jump(kind, world);
                        return RigState::Air;
                    }
                    Some(Action::JumpPrep { t: t - dt, kind })
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
        if desired.abs() > 1.0 && !matches!(self.action, Some(Action::Punch { .. })) {
            self.facing = desired.signum();
        }
        self.pos.y = surf.y - d.stance;
        self.vel.y = 0.0;

        let (lim0, lim1) = if surf.walled {
            (surf.x0 + 4.0 * s, surf.x1 - 4.0 * s)
        } else {
            (world.work.l + 8.0 * s, world.work.r - 8.0 * s)
        };
        if self.pos.x < lim0 || self.pos.x > lim1 {
            self.pos.x = self.pos.x.clamp(lim0, lim1);
            self.vel.x = 0.0;
            if matches!(self.action, Some(Action::Walk { .. })) {
                self.action = None;
            }
        }
        // Walked off the edge of a window.
        if !surf.walled && (self.pos.x < surf.x0 - 3.0 * s || self.pos.x > surf.x1 + 3.0 * s) {
            self.start_fall();
        }
        RigState::Stand { ground_y: surf.y, sit, gesture }
    }

    fn jump(&mut self, kind: JumpKind, world: &World) {
        let s = self.d.s;
        let foot = v2(self.pos.x, self.pos.y + self.d.stance);
        let from = match self.mode {
            Mode::Ground(sup) => Some(sup),
            _ => None,
        };
        match kind {
            JumpKind::To { target, support } => {
                self.launch(target, 35.0 * s);
                self.jump_target = Some(support);
            }
            JumpKind::Flip => {
                let mut target = foot - v2(self.facing * 18.0 * s, 0.0);
                target.x = target.x.clamp(world.work.l + 10.0 * s, world.work.r - 10.0 * s);
                let flight = self.launch(target, 75.0 * s);
                self.spin_rate = -self.facing * TAU / flight;
                // Land back where we took off, even inside a window.
                self.jump_target = from;
            }
            JumpKind::Cursor { secs } => {
                // Aim so the hands reach the cursor right at the top of the jump.
                let target = self.cursor + v2(0.0, self.d.reach() + self.d.stance);
                let g = GRAVITY * s;
                let up = (foot.y - target.y).max(10.0 * s);
                let t = (2.0 * up / g).sqrt();
                self.vel = v2((target.x - foot.x) / t, -(2.0 * g * up).sqrt());
                self.mode = Mode::Air;
                self.action = None;
                self.reset_air_state();
                self.chasing_cursor = Some(secs);
                self.jump_target = from;
            }
        }
    }

    /// Ballistic jump that lands the feet on `target`. Returns flight time.
    fn launch(&mut self, target: Vec2, extra: f32) -> f32 {
        let g = GRAVITY * self.d.s;
        let foot = v2(self.pos.x, self.pos.y + self.d.stance);
        let apex = foot.y.min(target.y) - (extra + (target.x - foot.x).abs() * 0.12);
        let up = (foot.y - apex).max(1.0);
        let down = (target.y - apex).max(1.0);
        let flight = (2.0 * up / g).sqrt() + (2.0 * down / g).sqrt();
        self.vel = v2((target.x - foot.x) / flight, -(2.0 * g * up).sqrt());
        self.mode = Mode::Air;
        self.action = None;
        self.reset_air_state();
        flight
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
        self.reset_air_state();
        self.pending_climb = Some((win, side));
        true
    }

    fn enter_climb(&mut self, world: &World, win: WinId, side: Side) {
        self.mode = Mode::Climb { win, side };
        self.layer = Layer::On(win);
        self.reset_air_state();
        self.anchor = window_origin(world, win).map(|o| (win, o));
        self.vel = Vec2::ZERO;
        self.action = None;
        self.facing = -side.outward();
    }

    fn update_air(&mut self, dt: f32, world: &World) -> Option<f32> {
        let d = self.d;
        let s = d.s;
        let prev_foot = self.pos.y + d.stance;
        self.vel.y += GRAVITY * s * dt;
        self.pos += self.vel * dt;
        if self.spin_rate != 0.0 {
            self.spin = (self.spin + self.spin_rate * dt).clamp(-TAU, TAU);
        }

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

        // Jumping at the cursor: grab it if the hands get close enough.
        if let Some(secs) = self.chasing_cursor {
            let hands = self.pos - v2(0.0, d.reach());
            if self.allow_cursor && (self.cursor - hands).len() < 26.0 * s {
                self.start_hang(secs);
                return None;
            }
            if self.vel.y > 250.0 * s {
                self.chasing_cursor = None;
            }
        }

        // Jumped up to catch a wall?
        if let Some((win, side)) = self.pending_climb {
            if self.vel.y > -80.0 * s {
                if let (Some(wi), Some(wx)) = (world.window(win), world.wall_x(win, side)) {
                    let r = wi.rect;
                    let stand = wx + side.outward() * 9.0 * s;
                    if (self.pos.x - stand).abs() < 16.0 * s && self.pos.y <= r.b + d.reach() && self.pos.y >= r.t {
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
                if world.can_land(&surf, self.layer, self.jump_target)
                    && self.pos.x >= surf.x0 - 2.0
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
        let s = self.d.s;
        if !self.force_front {
            self.layer = Layer::for_support(surf.support, self.layer);
        }
        let spun = self.spin;
        self.mode = Mode::Ground(surf.support);
        self.pos.y = surf.y - self.d.stance;
        self.vel.y = 0.0;
        self.reset_air_state();
        // Way too hard (or botched a flip): tumble.
        if impact > 2300.0 * s || (spun.abs() > 0.6 && spun.abs() < TAU - 0.6) {
            self.start_ragdoll(v2(self.vel.x * 0.8, -impact * 0.15));
            return;
        }
        self.vel.x *= 0.35;
        self.rig.on_land(impact);
        self.anchor = surf.support.window().and_then(|id| window_origin(world, id).map(|o| (id, o)));
        self.action = Some(if impact > 1300.0 * s { Action::Recover(0.5) } else { Action::Idle(0.15) });
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
