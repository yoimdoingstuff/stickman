//! The stickman's body: physics and the low-level actions (walk, jump, flip,
//! climb, mantle, punch, hang off the cursor, go ragdoll). The brain picks
//! *what* to do; this module does it, and the rig turns it into a pose.

use std::f32::consts::{PI, TAU};

use crate::brain::{Intent, PeerView, Perception};
use crate::math::*;
use crate::ragdoll::{Ragdoll, FOOT, HAND};
use crate::rig::{Gesture, Pose, Rig, RigInput, RigState, WalkStyle};
use crate::world::*;

/// Pixels/s² at scale 1 and gravity setting 1.0.
pub const GRAVITY: f32 = 1900.0;
/// Highest jump the brain will attempt, at scale 1 and normal gravity.
pub const MAX_JUMP: f32 = 230.0;
/// Below this speed a released/dropped body lands on its feet instead of flopping.
const GENTLE: f32 = 700.0;
/// Health per second recovered while awake / asleep.
const HEAL: f32 = 0.012;
const HEAL_ASLEEP: f32 = 0.04;
/// Below these, it limps / can only crawl.
pub const HURT: f32 = 0.5;
pub const WEAK: f32 = 0.2;

/// Pinned flat between a window and the screen edge (or floor).
#[derive(Clone, Copy, Debug)]
struct Squash {
    horizontal: bool,
    /// The edge (x) or floor (y) it's flattened against.
    line: f32,
    /// Which way it pops out afterwards.
    dir: f32,
    t: f32,
}

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
    /// Climbing a window's side wall; `inner` = the inside of the wall.
    Climb { win: WinId, side: Side, inner: bool },
    /// Diving through the taskbar to pop out at another window.
    Portal { t: f32, target: Support, dir: Vec2 },
    /// Pulling up over a window's top edge. `from` is relative to the window.
    Mantle { win: WinId, side: Side, t: f32, from: Vec2 },
    /// Limp physics body (thrown, knocked over, picked up, hanging on the cursor).
    Ragdoll,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    /// Standing around. `fidget` plays on top; with no fidget it looks around.
    Idle { t: f32, fidget: Gesture, look: f32 },
    Walk { x: f32, speed: f32, style: WalkStyle },
    Sit(f32),
    /// Hold a gesture for a while (wave, dance, sleep, push-ups, ...).
    Pose { t: f32, gesture: Gesture },
    JumpPrep { t: f32, kind: JumpKind },
    ClimbApproach { win: WinId, side: Side, inner: bool },
    /// Walk to `x`, then dive into the taskbar in direction `dir`.
    PortalApproach { x: f32, dir: Vec2, target: Support },
    Recover(f32),
    Punch { t: f32, target: u32, dir: f32, fired: bool },
    Kick { t: f32, target: Option<u32>, dir: f32, fired: bool },
    /// Walk to a ledge, then sit on it facing the drop.
    EdgeSit { t: f32, edge: f32, dir: f32 },
}

fn idle(t: f32) -> Action {
    Action::Idle { t, fidget: Gesture::None, look: 1.0 }
}

#[derive(Clone, Copy, Debug)]
enum JumpKind {
    To { target: Vec2, support: Support },
    Flip,
    Cartwheel,
    Cursor { secs: f32 },
}

/// Things that happened during an update that the app needs to know about.
#[derive(Default)]
pub struct Events {
    pub landed: Option<f32>,
    /// (peer id, impulse, max distance) when a punch or kick lands.
    pub hit: Option<(u32, Vec2, f32)>,
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
    sleeping: bool,
    /// Hidden health, 0..1. Low health = limping, then crawling.
    pub health: f32,
    /// Gravity multiplier from settings (1.0 = normal).
    pub gravity: f32,
    squash: Option<Squash>,
    /// Ragdoll velocity last step, to detect slams.
    last_rag_vel: Vec2,
    /// Window we're walking around inside of (its walls hold us in).
    room: Option<WinId>,
    /// Windows we were already overlapping when we took off (not solid).
    air_ignore: Option<Vec<WinId>>,
    pub rig: Rig,
}

/// Launch velocity and flight time for a jump from `foot` that lands at
/// `target`, peaking `extra` above the higher of the two.
pub fn ballistic(foot: Vec2, target: Vec2, extra: f32, g: f32) -> (Vec2, f32) {
    let apex = foot.y.min(target.y) - (extra + (target.x - foot.x).abs() * 0.12);
    let up = (foot.y - apex).max(1.0);
    let down = (target.y - apex).max(1.0);
    let flight = (2.0 * up / g).sqrt() + (2.0 * down / g).sqrt();
    (v2((target.x - foot.x) / flight, -(2.0 * g * up).sqrt()), flight)
}

fn window_origin(world: &World, id: WinId) -> Option<Vec2> {
    world.window(id).map(|w| v2(w.rect.l, w.rect.t))
}

fn map_pose(p: &Pose, f: impl Fn(Vec2) -> Vec2) -> Pose {
    Pose {
        hip: f(p.hip),
        chest: f(p.chest),
        neck: f(p.neck),
        head: f(p.head),
        knees: [f(p.knees[0]), f(p.knees[1])],
        feet: [f(p.feet[0]), f(p.feet[1])],
        elbows: [f(p.elbows[0]), f(p.elbows[1])],
        hands: [f(p.hands[0]), f(p.hands[1])],
    }
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
            sleeping: false,
            health: 1.0,
            gravity: 1.0,
            squash: None,
            last_rag_vel: Vec2::ZERO,
            room: None,
            air_ignore: None,
            rig: Rig::new(),
        }
    }

    /// Current gravity in px/s².
    fn g(&self) -> f32 {
        GRAVITY * self.d.s * self.gravity
    }

    /// Same leg strength, so lower gravity means higher jumps.
    fn max_jump(&self) -> f32 {
        (MAX_JUMP * self.d.s / self.gravity.max(0.1)).min(600.0 * self.d.s)
    }

    fn damage(&mut self, amount: f32) {
        self.health = (self.health - amount.max(0.0)).max(0.0);
    }

    fn walk_style(&self, wanted: WalkStyle) -> WalkStyle {
        if self.health < WEAK {
            WalkStyle::Crawl
        } else if self.health < HURT {
            WalkStyle::Limp
        } else {
            wanted
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
                max_jump: self.max_jump(),
                health: self.health,
                layer: self.layer,
                gravity: self.g(),
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
            Mode::Air => self.pending_climb.map(|(id, _)| id).or(self.room),
            Mode::Portal { .. } => None,
        }
    }

    pub fn in_portal(&self) -> bool {
        matches!(self.mode, Mode::Portal { .. })
    }

    /// Fully inside the taskbar, between leaving and arriving.
    pub fn hidden(&self) -> bool {
        matches!(self.mode, Mode::Portal { t, .. } if t >= 0.45)
    }

    pub fn is_resting(&self) -> bool {
        matches!(self.mode, Mode::Ground(_))
            && matches!(
                self.action,
                Some(Action::Sit(_))
                    | Some(Action::Idle { fidget: Gesture::None, .. })
                    | Some(Action::Pose { gesture: Gesture::Sleep, .. })
                    | None
            )
            && self.vel.x.abs() < 1.0
    }

    /// Head position and time, while asleep (for drawing Z's).
    pub fn zzz(&self) -> Option<(Vec2, f32)> {
        (self.sleeping && self.ragdoll.is_none()).then(|| (self.rig.pose.head, self.time))
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
        let p = match &self.ragdoll {
            Some(r) => r.pose(&self.d),
            None if self.spin != 0.0 => {
                let p = self.rig.pose;
                let pivot = (p.hip + p.chest) * 0.5;
                let a = self.spin;
                map_pose(&p, |v| rotate(v, pivot, a))
            }
            None => self.rig.pose,
        };
        match self.squash {
            // Flattened against the edge/floor like a cartoon pancake.
            Some(sq) => {
                let k = if sq.t < 0.08 { 1.0 - (sq.t / 0.08) * 0.8 } else { 0.2 + (sq.t * 30.0).sin() * 0.02 };
                if sq.horizontal {
                    map_pose(&p, |v| v2(sq.line + (v.x - sq.line) * k, v.y))
                } else {
                    map_pose(&p, |v| v2(v.x + (v.x - p.hip.x) * (1.0 - k) * 0.6, sq.line + (v.y - sq.line) * k))
                }
            }
            None => p,
        }
    }

    pub fn is_squashed(&self) -> bool {
        self.squash.is_some()
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
                    self.room = Some(id);
                    self.jump_target = Some(Support::Inside(id));
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

    fn start_squash(&mut self, horizontal: bool, line: f32, dir: f32) {
        if self.squash.is_some() {
            return;
        }
        self.damage(0.35);
        self.squash = Some(Squash { horizontal, line, dir, t: 0.0 });
        self.hang = 0.0;
        self.user_pin = None;
        if let Some(r) = &mut self.ragdoll {
            r.clear_pins();
        }
    }

    /// A window moved from `old` to `new` this frame. If its edge ran into
    /// us, get pushed along — or squashed flat if there's nowhere to go.
    pub fn shove(&mut self, world: &World, id: WinId, old: Rect, new: Rect, dt: f32) {
        if self.squash.is_some()
            || self.user_pin.is_some()
            || self.layer == Layer::Behind
            || self.attached_window() == Some(id)
            || dt <= 0.0
        {
            return;
        }
        let s = self.d.s;
        let c = self.center();
        let half = 6.0 * s;
        let (top, bottom) = (c.y - 38.0 * s, c.y + self.d.stance);
        let w = world.work;
        let v_overlap = new.t < bottom && new.b > top;
        let h_overlap = new.l < c.x + half && new.r > c.x - half;

        if v_overlap && new.r > old.r && old.r <= c.x - half + 2.0 && new.r > c.x - half {
            // Right edge sweeping right into us.
            let target = new.r + half;
            if target > w.r - half {
                self.start_squash(true, w.r, -1.0);
            } else {
                self.push_x(target - c.x, (new.r - old.r) / dt);
            }
        } else if v_overlap && new.l < old.l && old.l >= c.x + half - 2.0 && new.l < c.x + half {
            // Left edge sweeping left into us.
            let target = new.l - half;
            if target < w.l + half {
                self.start_squash(true, w.l, 1.0);
            } else {
                self.push_x(target - c.x, (new.l - old.l) / dt);
            }
        } else if h_overlap && new.b > old.b && old.b <= top + 2.0 && new.b > top {
            // Bottom edge coming down on our head.
            let ground = match self.mode {
                Mode::Ground(sup) => world.surface(sup).map(|f| f.y),
                _ => None,
            }
            .unwrap_or(w.b);
            if ground - new.b < (bottom - top) * 0.6 {
                self.start_squash(false, ground, -1.0);
            } else {
                let dy = new.b - top;
                match &mut self.ragdoll {
                    Some(r) => r.shove(v2(0.0, dy)),
                    None if self.mode == Mode::Air => self.pos.y += dy,
                    None => {}
                }
            }
        }
    }

    fn push_x(&mut self, dx: f32, edge_speed: f32) {
        let s = self.d.s;
        match &mut self.ragdoll {
            Some(r) => r.shove(v2(dx, 0.0)),
            None => {
                self.pos.x += dx;
                self.rig.shift(v2(dx, 0.0));
                // Shoved hard: knocked off its feet.
                if edge_speed.abs() > 1400.0 * s {
                    self.knock(v2(edge_speed * 0.5, -250.0 * s));
                }
            }
        }
    }

    /// Hit by someone else.
    pub fn knock(&mut self, impulse: Vec2) {
        self.hang = 0.0;
        self.damage(impulse.len() / (4500.0 * self.d.s));
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
        self.room = None;
        self.reset_air_state();
    }

    pub fn apply(&mut self, intent: Intent, world: &World) {
        if !matches!(self.mode, Mode::Ground(_)) {
            return;
        }
        // Too hurt for anything acrobatic: rest instead.
        let allowed = match intent {
            Intent::Idle { .. } | Intent::Walk { .. } | Intent::Sit { .. } => true,
            Intent::Pose { gesture: Gesture::Sleep, .. } => true,
            _ if self.health < WEAK => false,
            Intent::Pose { gesture: Gesture::PushUp, .. }
            | Intent::Flip
            | Intent::Cartwheel
            | Intent::GrabCursor { .. }
            | Intent::Climb { .. }
            | Intent::Portal { .. }
            | Intent::Kick { .. } => self.health >= HURT,
            _ => true,
        };
        if !allowed {
            self.action = Some(idle(1.5));
            return;
        }
        self.action = Some(match intent {
            Intent::Idle { secs, fidget } => Action::Idle { t: secs, fidget, look: 0.8 },
            Intent::Walk { x, speed, style } => Action::Walk { x, speed, style },
            Intent::Sit { secs } => Action::Sit(secs),
            Intent::Pose { secs, gesture } => Action::Pose { t: secs, gesture },
            Intent::Kick { target, dir } => {
                self.facing = dir;
                Action::Kick { t: 0.0, target, dir, fired: false }
            }
            Intent::EdgeSit { secs, edge, dir } => Action::EdgeSit { t: secs, edge, dir },
            Intent::Cartwheel => Action::JumpPrep { t: 0.15, kind: JumpKind::Cartwheel },
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
            Intent::Climb { win, side, inner } => Action::ClimbApproach { win, side, inner },
            Intent::Portal { target } => {
                let s = self.d.s;
                let floor = world.surface(Support::Floor);
                match (world.portal_edge(), floor) {
                    (Some(Edge::Bottom), _) => Action::PortalApproach { x: self.pos.x, dir: v2(0.0, 1.0), target },
                    (Some(Edge::Left), Some(f)) => {
                        Action::PortalApproach { x: f.x0 + 6.0 * s, dir: v2(-1.0, 0.0), target }
                    }
                    (Some(Edge::Right), Some(f)) => {
                        Action::PortalApproach { x: f.x1 - 6.0 * s, dir: v2(1.0, 0.0), target }
                    }
                    _ => idle(0.5),
                }
            }
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
                idle(0.6)
            }
        });
    }

    // ---------- simulation ----------

    /// Advance one physics step.
    pub fn update(&mut self, dt: f32, world: &World, cursor: Vec2, cursor_vel: Vec2) -> Events {
        self.time += dt;
        self.crouch = 0.0;
        let heal = if self.sleeping { HEAL_ASLEEP } else { HEAL };
        self.health = (self.health + heal * dt).min(1.0);
        self.sleeping = false;
        self.cursor = cursor;
        self.cursor_vel = cursor_vel;
        let mut ev = Events::default();

        // Squashed flat: hold it for a moment, then pop out limp.
        if let Some(mut sq) = self.squash {
            sq.t += dt;
            if sq.t < 1.3 {
                self.squash = Some(sq);
                return ev;
            }
            self.squash = None;
            let s = self.d.s;
            let pop = (if sq.horizontal { v2(sq.dir * 350.0, -300.0) } else { v2(self.facing * 120.0, -450.0) }) * s;
            match &mut self.ragdoll {
                Some(r) => r.add_velocity(pop),
                None => self.start_ragdoll(pop),
            }
            return ev;
        }
        let mode = self.mode;
        let state = match mode {
            Mode::Ground(support) => self.update_ground(dt, world, support, &mut ev),
            Mode::Air => {
                ev.landed = self.update_air(dt, world);
                RigState::Air
            }
            Mode::Climb { win, side, inner } => self.update_climb(dt, world, win, side, inner),
            Mode::Mantle { win, side, t, from } => self.update_mantle(dt, world, win, side, t, from),
            Mode::Portal { t, target, dir } => {
                let t = t + dt;
                if t < 0.45 {
                    // Diving into the taskbar (the taskbar covers us as we go in).
                    self.vel = dir * 260.0 * self.d.s;
                    self.pos += self.vel * dt;
                }
                if t >= 0.85 {
                    self.exit_portal(world, target);
                } else {
                    self.mode = Mode::Portal { t, target, dir };
                }
                RigState::Air
            }
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

    /// Pop out of the taskbar at the target window (on top of it, or inside).
    fn exit_portal(&mut self, world: &World, target: Support) {
        let s = self.d.s;
        let before = self.pos;
        self.mode = Mode::Air;
        self.action = None;
        self.reset_air_state();
        self.room = None;
        match world.surface(target) {
            None => {
                // Window's gone: hop back out of the taskbar.
                self.vel = v2(0.0, -700.0 * s);
                self.layer = Layer::Front;
            }
            Some(surf) => {
                let x = (surf.x0 + surf.x1) * 0.5 + (self.time * 13.7).sin() * (surf.x1 - surf.x0) * 0.3;
                self.jump_target = Some(target);
                match target {
                    Support::Inside(id) => {
                        let top = world.window(id).map(|w| w.rect.t.max(world.work.t)).unwrap_or(surf.y - 200.0 * s);
                        self.pos = v2(x, (top + 45.0 * s).min(surf.y - self.d.stance));
                        self.vel = Vec2::ZERO;
                        self.layer = Layer::On(id);
                        self.room = Some(id);
                    }
                    _ => {
                        self.pos = v2(x, surf.y - self.d.stance - 70.0 * s);
                        self.vel = v2(0.0, -250.0 * s);
                        self.layer = target.window().map(Layer::On).unwrap_or(Layer::Front);
                    }
                }
            }
        }
        self.rig.shift(self.pos - before);
    }

    fn reset_air_state(&mut self) {
        self.air_ignore = None;
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
        self.last_rag_vel = vel;
        self.mode = Mode::Ragdoll;
        self.action = None;
        self.reset_air_state();
    }

    /// Ragdoll -> controlled fall (lands on its feet).
    fn air_from_ragdoll(&mut self) {
        self.room = None;
        if let Some(r) = self.ragdoll.take() {
            let pose = r.pose(&self.d);
            self.pos = r.hip();
            self.vel = r.velocity();
            self.room = r.room;
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
        let gravity = self.g();
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
                r.step(dt, &d, world, layer, gravity);
                self.pos = r.hip();
                (r.velocity(), r.settled())
            }
        };
        self.vel = vel;
        // A sudden change in speed means it slammed into something.
        let dv = (vel - self.last_rag_vel).len();
        self.last_rag_vel = vel;
        if !held && !hanging && dv > 1100.0 * s {
            self.damage((dv - 1100.0 * s) / (9000.0 * s));
        }

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
        let mut style = self.walk_style(WalkStyle::Normal);
        self.room = match support {
            Support::Inside(id) => Some(id),
            _ => None,
        };
        if let Some(action) = self.action {
            let next = match action {
                Action::PortalApproach { x, dir, target } => {
                    let dx = x - self.pos.x;
                    if dx.abs() > 2.5 * s {
                        desired = dx.signum() * 120.0 * s;
                        Some(action)
                    } else {
                        self.mode = Mode::Portal { t: 0.0, target, dir };
                        self.action = None;
                        self.reset_air_state();
                        self.room = None;
                        return RigState::Air;
                    }
                }
                Action::Idle { t, fidget, look } => {
                    gesture = fidget;
                    let mut look = look - dt;
                    if look <= 0.0 && fidget == Gesture::None {
                        // Glance around; prefer looking at the cursor if it's close.
                        look = 1.1 + (self.time * 7.3).sin().abs() * 1.4;
                        let to = self.cursor - self.pos;
                        self.facing = if to.len() < 300.0 * s && to.x.abs() > 4.0 * s {
                            to.x.signum()
                        } else {
                            -self.facing
                        };
                    }
                    (t > dt).then_some(Action::Idle { t: t - dt, fidget, look })
                }
                Action::Sit(t) => {
                    sit = true;
                    (t > dt).then(|| Action::Sit(t - dt))
                }
                Action::Pose { t, gesture: g } => {
                    gesture = g;
                    self.sleeping = g == Gesture::Sleep;
                    (t > dt).then_some(Action::Pose { t: t - dt, gesture: g })
                }
                Action::Kick { t, target, dir, fired } => {
                    let t = t + dt;
                    let ext = if t < 0.12 {
                        -(t / 0.12)
                    } else if t < 0.26 {
                        1.0
                    } else {
                        (1.0 - (t - 0.26) / 0.18).max(0.0)
                    };
                    gesture = Gesture::Kick(ext);
                    self.facing = dir;
                    let mut fired = fired;
                    if t >= 0.12 && !fired {
                        fired = true;
                        if let Some(id) = target {
                            ev.hit = Some((id, v2(dir * 950.0, -520.0) * s, 50.0 * s));
                        }
                    }
                    (t < 0.5).then_some(Action::Kick { t, target, dir, fired })
                }
                Action::EdgeSit { t, edge, dir } => {
                    let spot = edge - dir * 4.0 * s;
                    let dx = spot - self.pos.x;
                    if dx.abs() > 2.5 * s {
                        desired = dx.signum() * 90.0 * s;
                        Some(action)
                    } else {
                        self.pos.x = spot;
                        self.facing = dir;
                        gesture = Gesture::EdgeSit;
                        (t > dt).then_some(Action::EdgeSit { t: t - dt, edge, dir })
                    }
                }
                Action::Recover(t) => {
                    self.crouch = (t / 0.5).min(1.0) * 0.8;
                    (t > dt).then(|| Action::Recover(t - dt))
                }
                Action::Walk { x, speed, style: wanted } => {
                    style = self.walk_style(wanted);
                    // Hurt: forced down to limping/crawling speed.
                    let speed = if style != wanted { speed.min(style.speed() * s) } else { speed };
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
                        ev.hit = Some((target, v2(dir * 650.0, -380.0) * s, 40.0 * s));
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
                Action::ClimbApproach { win, side, inner } => match world.wall_x(win, side) {
                    None => None,
                    Some(wx) => {
                        let out = if inner { -side.outward() } else { side.outward() };
                        let stand = wx + out * 9.0 * s;
                        let dx = stand - self.pos.x;
                        if dx.abs() < 2.5 * s {
                            self.pos.x = stand;
                            self.vel.x = 0.0;
                            self.facing = -out;
                            self.action = None;
                            if inner {
                                // Inside a window the wall goes right down to the floor.
                                self.enter_climb(world, win, side, true);
                                return RigState::Air;
                            }
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
        if desired.abs() > 1.0 && !matches!(self.action, Some(Action::Punch { .. }) | Some(Action::Kick { .. })) {
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
        // Sitting / lying poses take priority over crawling.
        if sit || gesture.is_low() {
            style = WalkStyle::Normal;
        }
        RigState::Stand { ground_y: surf.y, sit, gesture, style }
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
            JumpKind::Cartwheel => {
                let mut target = foot + v2(self.facing * 80.0 * s, 0.0);
                target.x = target.x.clamp(world.work.l + 10.0 * s, world.work.r - 10.0 * s);
                let flight = self.launch(target, 30.0 * s);
                self.spin_rate = self.facing * TAU / flight;
                self.jump_target = from;
            }
            JumpKind::Cursor { secs } => {
                // Aim so the hands reach the cursor right at the top of the jump.
                let target = self.cursor + v2(0.0, self.d.reach() + self.d.stance);
                let g = self.g();
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
        let foot = v2(self.pos.x, self.pos.y + self.d.stance);
        let (vel, flight) = ballistic(foot, target, extra, self.g());
        self.vel = vel;
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
            self.enter_climb(world, win, side, false);
            return true;
        }
        let target_y = r.b + reach * 0.5;
        let h = self.pos.y - target_y;
        if h <= 0.0 || h > self.max_jump() {
            return false;
        }
        self.vel = v2(0.0, -(2.0 * self.g() * h).sqrt());
        self.mode = Mode::Air;
        self.reset_air_state();
        self.pending_climb = Some((win, side));
        true
    }

    fn enter_climb(&mut self, world: &World, win: WinId, side: Side, inner: bool) {
        self.mode = Mode::Climb { win, side, inner };
        self.layer = Layer::On(win);
        self.reset_air_state();
        self.anchor = window_origin(world, win).map(|o| (win, o));
        self.vel = Vec2::ZERO;
        self.action = None;
        self.facing = if inner { side.outward() } else { -side.outward() };
    }

    /// Windows are solid while flying: bonk your head on their bottoms and
    /// bounce off their sides (landing on tops is handled separately).
    fn collide_windows(&mut self, world: &World, prev_pos: Vec2) {
        let s = self.d.s;
        let stance = self.d.stance;
        let ignore = self
            .air_ignore
            .get_or_insert_with(|| world.overlapping(prev_pos + v2(0.0, stance), s))
            .clone();
        let pb = body_box(prev_pos + v2(0.0, stance), s);
        let head_above_hip = 62.0 * s - stance;
        for (id, r) in world.solids(self.layer) {
            if ignore.contains(&id) || Some(id) == self.room {
                continue;
            }
            let nb = body_box(self.pos + v2(0.0, stance), s);
            if !r.overlaps(&nb) || r.overlaps(&pb) || pb.b <= r.t + 1.0 {
                continue;
            }
            if pb.t >= r.b - 1.0 {
                self.pos.y = r.b + head_above_hip + 0.5;
                self.vel.y = self.vel.y.abs() * 0.2;
            } else if pb.r <= r.l + 1.0 {
                self.pos.x = r.l - 5.5 * s;
                self.vel.x = -self.vel.x.abs() * 0.3;
            } else if pb.l >= r.r - 1.0 {
                self.pos.x = r.r + 5.5 * s;
                self.vel.x = self.vel.x.abs() * 0.3;
            }
        }
        // Inside a window: its walls and top keep us in.
        if let Some(r) = self.room.and_then(|id| world.window(id)).map(|w| w.rect) {
            let (l, rr) = (r.l + 6.0 * s, r.r - 6.0 * s);
            if self.pos.x < l {
                self.pos.x = l;
                self.vel.x = self.vel.x.abs() * 0.3;
            } else if self.pos.x > rr {
                self.pos.x = rr;
                self.vel.x = -self.vel.x.abs() * 0.3;
            }
            let ceiling = r.t.max(world.work.t) + head_above_hip;
            if self.pos.y < ceiling {
                self.pos.y = ceiling;
                self.vel.y = self.vel.y.max(0.0);
            }
        }
    }

    fn update_air(&mut self, dt: f32, world: &World) -> Option<f32> {
        let d = self.d;
        let s = d.s;
        let prev_foot = self.pos.y + d.stance;
        let prev_pos = self.pos;
        self.vel.y += self.g() * dt;
        self.pos += self.vel * dt;
        self.collide_windows(world, prev_pos);
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
                        self.enter_climb(world, win, side, false);
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
        if impact > 1300.0 * s {
            self.damage((impact - 1300.0 * s) / (7000.0 * s));
        }
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
        self.action = Some(if impact > 1300.0 * s { Action::Recover(0.5) } else { idle(0.15) });
    }

    fn update_climb(&mut self, dt: f32, world: &World, win: WinId, side: Side, inner: bool) -> RigState {
        let s = self.d.s;
        let (Some(w), Some(wx)) = (world.window(win).copied(), world.wall_x(win, side)) else {
            self.start_fall();
            return RigState::Air;
        };
        self.carry(world, Some(win));
        let r = w.rect;
        // Outside: hang off the outer face. Inside: on the inner face.
        let out = if inner { -side.outward() } else { side.outward() };
        self.pos.x = wx + out * 7.0 * s;
        self.facing = -out;
        // Climb in little pulses rather than at a constant speed.
        let speed = 80.0 * s * (0.6 + 0.4 * (self.time * 8.0).sin().abs());
        self.pos.y -= speed * dt;
        self.vel = v2(0.0, -speed);
        if self.pos.y <= r.t + 4.0 * s {
            // Over the top edge, ending up standing on the window.
            self.facing = -side.outward();
            self.room = None;
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
            self.action = Some(idle(0.4));
        } else {
            self.mode = Mode::Mantle { win, side, t, from };
        }
        RigState::Mantle { wall_x: wx, top_y: r.t }
    }
}
