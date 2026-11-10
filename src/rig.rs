//! Procedural animation. No keyframes: every frame the pose is solved from
//! the body's physics state. Feet and hands either *plant* (stay put until
//! they're too far from where they should be, then take an arcing step) or
//! *follow* a target through a spring (for dangling / flailing). Knees and
//! elbows come from two-bone IK; the torso and head lag behind on springs.

use std::f32::consts::{PI, TAU};

use crate::math::*;
use crate::stickman::Dims;

/// Upper-body moves layered on top of standing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    None,
    Wave,
    Dance,
    /// -1..0 = winding up, 0..1 = arm extended.
    Punch(f32),
    /// -1..0 = chambering the knee, 0..1 = leg extended.
    Kick(f32),
    /// Arms up over the head.
    Stretch,
    Scratch,
    /// Arms crossed, tapping a foot.
    TapFoot,
    /// Shadowboxing jabs.
    Box,
    /// Floating cross-legged.
    Meditate,
    PushUp,
    Sleep,
    /// Sitting on a ledge with legs dangling over it (facing the drop).
    EdgeSit,
    /// Arms windmilling, trying not to fall off a moving window.
    Balance,
    /// Spinning on the spot with arms out.
    Spin,
    /// Baseball slide: low, leaning back, feet first.
    Slide,
    /// 0..1: hand going up for a high-five.
    HighFive(f32),
    /// Body language (see `Feeling`).
    Startle,
    Cheer,
    Fume,
    Slump,
    FistPump,
}

/// How it feels about something that just happened. It never says a word:
/// each one comes out as body language for a moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feeling {
    /// Jolts, arms flung up.
    Startled,
    /// Scratches its head, head tilted (trying something new).
    Puzzled,
    /// Arms up in a V, bouncing (a friend, a high-five, your praise).
    Happy,
    /// Fists clenched, stomping, leaning in.
    Angry,
    /// Slumped, head hanging (scolded, lost).
    Sad,
    /// Fist pump (that went better than expected).
    Proud,
}

impl Feeling {
    pub fn gesture(self) -> Gesture {
        match self {
            Feeling::Startled => Gesture::Startle,
            Feeling::Puzzled => Gesture::Scratch,
            Feeling::Happy => Gesture::Cheer,
            Feeling::Angry => Gesture::Fume,
            Feeling::Sad => Gesture::Slump,
            Feeling::Proud => Gesture::FistPump,
        }
    }

    /// How long it shows.
    pub fn secs(self) -> f32 {
        match self {
            Feeling::Startled => 0.6,
            Feeling::Puzzled => 1.4,
            Feeling::Happy => 1.3,
            Feeling::Angry => 1.4,
            Feeling::Sad => 2.2,
            Feeling::Proud => 0.9,
        }
    }
}

impl Gesture {
    /// Poses that bring the hips down to the ground.
    pub fn is_low(self) -> bool {
        matches!(self, Gesture::Meditate | Gesture::PushUp | Gesture::Sleep | Gesture::EdgeSit)
    }
}

/// How it walks. Each style is a set of tweaks to the same procedural gait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkStyle {
    Normal,
    /// Crouched tiptoe with hands up, cartoon-burglar style.
    Sneak,
    /// Leaning back, big arm swing, bouncy.
    Strut,
    /// High knees, stiff arms.
    March,
    /// Hoppy.
    Skip,
    /// Tired: small slow steps, slumped.
    Shuffle,
    /// Hurt: one leg favoured, hip dips.
    Limp,
    /// Badly hurt: on hands and knees.
    Crawl,
    /// Gliding backwards (the stickman faces away from where it's going).
    Moonwalk,
    /// Running away screaming, arms flailing over its head.
    Panic,
}

impl WalkStyle {
    /// Natural walking speed at scale 1 (px/s).
    pub fn speed(self) -> f32 {
        match self {
            WalkStyle::Normal => 85.0,
            WalkStyle::Sneak => 50.0,
            WalkStyle::Strut => 95.0,
            WalkStyle::March => 75.0,
            WalkStyle::Skip => 140.0,
            WalkStyle::Shuffle => 45.0,
            WalkStyle::Limp => 40.0,
            WalkStyle::Crawl => 22.0,
            WalkStyle::Moonwalk => 60.0,
            WalkStyle::Panic => 380.0,
        }
    }
}

/// Gait tweaks: hip drop (px at scale 1), step lift/length/duration
/// multipliers, forward lean (rad), arm swing multiplier, hop amount.
struct Gait {
    hip: f32,
    lift: f32,
    stride: f32,
    dur: f32,
    lean: f32,
    swing: f32,
    bounce: f32,
}

fn gait(style: WalkStyle) -> Gait {
    let g = |hip, lift, stride, dur, lean, swing, bounce| Gait { hip, lift, stride, dur, lean, swing, bounce };
    match style {
        WalkStyle::Normal | WalkStyle::Crawl => g(0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 0.0),
        WalkStyle::Sneak => g(7.0, 0.7, 0.8, 1.4, 0.22, 0.2, 0.0),
        WalkStyle::Strut => g(-1.0, 1.2, 1.1, 1.0, -0.1, 1.6, 1.2),
        WalkStyle::March => g(0.0, 2.6, 1.0, 1.3, -0.04, 1.8, 0.4),
        WalkStyle::Skip => g(0.0, 1.6, 1.3, 0.9, 0.05, 1.2, 3.0),
        WalkStyle::Shuffle => g(2.0, 0.35, 0.6, 1.5, 0.18, 0.3, 0.0),
        WalkStyle::Limp => g(1.0, 0.6, 0.7, 1.5, 0.12, 0.5, 0.0),
        WalkStyle::Moonwalk => g(0.5, 0.45, 1.2, 1.3, -0.05, 0.4, 0.0),
        WalkStyle::Panic => g(1.0, 1.4, 1.2, 0.8, 0.25, 0.0, 0.5),
    }
}

#[derive(Clone, Copy, Debug)]
pub enum RigState {
    Stand { ground_y: f32, sit: bool, gesture: Gesture, style: WalkStyle },
    Air,
    Climb { wall_x: f32 },
    Mantle { wall_x: f32, top_y: f32 },
}

impl RigState {
    fn kind(&self) -> u8 {
        match self {
            RigState::Stand { .. } => 0,
            RigState::Air => 1,
            RigState::Climb { .. } => 2,
            RigState::Mantle { .. } => 3,
        }
    }
}

pub struct RigInput {
    pub hip: Vec2,
    pub vel: Vec2,
    /// +1 facing right, -1 facing left.
    pub facing: f32,
    pub state: RigState,
    /// 0..1, how much to crouch (jump anticipation, hard landings).
    pub crouch: f32,
    pub time: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Pose {
    pub hip: Vec2,
    pub chest: Vec2,
    pub neck: Vec2,
    pub head: Vec2,
    pub knees: [Vec2; 2],
    pub feet: [Vec2; 2],
    pub elbows: [Vec2; 2],
    pub hands: [Vec2; 2],
}

#[derive(Clone, Copy, Debug, Default)]
struct Limb {
    pos: Vec2,
    vel: Vec2,
    from: Vec2,
    to: Vec2,
    t: f32,
    dur: f32,
    lift: Vec2,
    stepping: bool,
}

impl Limb {
    fn start_step(&mut self, to: Vec2, dur: f32, lift: Vec2) {
        self.from = self.pos;
        self.to = to;
        self.t = 0.0;
        self.dur = dur.max(0.02);
        self.lift = lift;
        self.stepping = true;
    }

    fn advance(&mut self, dt: f32) {
        if self.stepping {
            self.t = (self.t + dt / self.dur).min(1.0);
            self.pos = self.from.lerp(self.to, smoothstep(self.t)) + self.lift * (self.t * PI).sin();
            if self.t >= 1.0 {
                self.stepping = false;
                self.pos = self.to;
            }
        }
        self.vel = Vec2::ZERO;
    }

    fn follow(&mut self, target: Vec2, freq: f32, damping: f32, dt: f32) {
        self.stepping = false;
        let w = TAU * freq;
        self.vel += ((target - self.pos) * (w * w) - self.vel * (2.0 * damping * w)) * dt;
        self.pos += self.vel * dt;
    }

    fn lift_amount(&self) -> f32 {
        if self.stepping {
            (self.t * PI).sin()
        } else {
            0.0
        }
    }

    fn shift(&mut self, d: Vec2) {
        self.pos += d;
        self.from += d;
        self.to += d;
    }
}

/// Step whichever planted limb of the pair is furthest from where it should be.
#[allow(clippy::too_many_arguments)]
fn step_pair(
    limbs: &mut [Limb; 2],
    ideal: [Vec2; 2],
    lead: Vec2,
    threshold: f32,
    dur: f32,
    lift: Vec2,
    snap: bool,
    dt: f32,
) {
    for l in limbs.iter_mut() {
        l.advance(dt);
    }
    if snap {
        for i in 0..2 {
            limbs[i].start_step(ideal[i], dur * 0.5, lift * 0.5);
        }
        return;
    }
    if limbs[0].stepping || limbs[1].stepping {
        return;
    }
    let e0 = (limbs[0].pos - ideal[0]).len();
    let e1 = (limbs[1].pos - ideal[1]).len();
    let i = if e0 >= e1 { 0 } else { 1 };
    if e0.max(e1) > threshold {
        limbs[i].start_step(ideal[i] + lead, dur, lift);
    }
}

/// Smooth, never-repeating wobble in roughly -1..1 (three detuned sines).
/// `k` offsets it so different body parts and stickmen don't move in sync.
fn wobble(t: f32, k: f32) -> f32 {
    (t + k).sin() * 0.5 + (t * 2.31 + k * 1.7).sin() * 0.3 + (t * 4.13 + k * 2.9).sin() * 0.2
}

/// Two-bone IK. Returns (middle joint, end point). `bend` picks which way the
/// joint points (+1 = toward +x when the limb hangs down).
fn ik(root: Vec2, target: Vec2, a: f32, b: f32, bend: f32) -> (Vec2, Vec2) {
    let d = target - root;
    let dist = d.len();
    let dc = dist.clamp((a - b).abs() + 0.01, (a + b) * 0.999);
    let dir = if dist > 1e-4 { d / dist } else { v2(0.0, 1.0) };
    let end = root + dir * dc;
    let cos_a = ((a * a + dc * dc - b * b) / (2.0 * a * dc)).clamp(-1.0, 1.0);
    let ang = dir.y.atan2(dir.x) - bend.signum() * cos_a.acos();
    (root + v2(ang.cos(), ang.sin()) * a, end)
}

pub struct Rig {
    feet: [Limb; 2],
    hands: [Limb; 2],
    lean: Spring,
    crouch: Spring,
    squash: Spring,
    sit: Spring,
    tilt: Spring,
    kind: u8,
    init: bool,
    /// Per-stickman phase so idles don't look identical.
    seed: f32,
    /// Which way it sits on the ground this time: legs out leaning back on
    /// its hands, or knees up with hands on knees. Picked when it sits down.
    legs_out: bool,
    was_sitting: bool,
    /// Walk cycle position, 0..1 (one full cycle = two steps).
    phase: f32,
    pub pose: Pose,
}

impl Rig {
    pub fn new() -> Self {
        Rig {
            feet: [Limb::default(); 2],
            hands: [Limb::default(); 2],
            lean: Spring::default(),
            crouch: Spring::default(),
            squash: Spring::default(),
            sit: Spring::default(),
            tilt: Spring::default(),
            kind: 255,
            init: false,
            seed: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| (d.subsec_nanos() % 100_000) as f32 * 0.001)
                .unwrap_or(0.0),
            legs_out: false,
            was_sitting: false,
            phase: 0.0,
            pose: Pose::default(),
        }
    }

    /// Move everything planted along with the window we're standing on.
    pub fn shift(&mut self, d: Vec2) {
        for l in self.feet.iter_mut().chain(self.hands.iter_mut()) {
            l.shift(d);
        }
    }

    /// Take over from a ragdoll: limbs start where the ragdoll left them and
    /// the springs pull the body back upright, which reads as getting up.
    pub fn adopt(&mut self, pose: &Pose) {
        for i in 0..2 {
            self.feet[i] = Limb { pos: pose.feet[i], ..Limb::default() };
            self.hands[i] = Limb { pos: pose.hands[i], ..Limb::default() };
        }
        let up = pose.chest - pose.hip;
        let angle = up.x.atan2(-up.y);
        self.lean = Spring { x: angle, v: 0.0 };
        self.tilt = Spring { x: angle, v: 0.0 };
        self.kind = 255;
        self.init = true;
        self.pose = *pose;
    }

    /// One frame of the walk/run cycle. Each foot spends part of the cycle
    /// planted (it never slides) and the rest swinging forward in an arc that
    /// lifts early (knee up) and reaches out late, landing ahead of the hip.
    /// Returns how far to lower the hips: walking, the body vaults over the
    /// planted leg like an inverted pendulum (highest mid-step, lowest with
    /// both feet down); running, it compresses on each landing and floats
    /// between steps.
    #[allow(clippy::too_many_arguments)]
    fn gait_step(&mut self, hip: Vec2, vel_x: f32, ground_y: f32, g: &Gait, d: &Dims, dt: f32, run: f32) -> f32 {
        let s = d.s;
        let speed = vel_x.abs().max(1.0);
        let dir = if vel_x >= 0.0 { 1.0 } else { -1.0 };
        // Ground covered by one full cycle (two steps): longer when faster.
        let stride = ((30.0 * s + speed * 0.22) * g.stride).clamp(24.0 * s, 110.0 * s);
        // Fraction of the cycle each foot is on the ground (walk > half,
        // run < half, so running has moments with both feet in the air).
        let duty = 0.62 - 0.27 * run;
        self.phase = (self.phase + speed * dt / stride).fract();
        let half = stride * duty * 0.5;
        let lift_h = (3.0 * s + speed * 0.025) * g.lift * (1.0 + run * 0.6);

        let leg = d.stance + s;
        let mut pendulum = f32::MAX;
        let mut compress = 0.0f32;
        let mut grounded = false;
        for i in 0..2 {
            let p = (self.phase + i as f32 * 0.5).fract();
            let foot = &mut self.feet[i];
            if p < duty {
                // Stance: plant where the swing was heading, then stay put.
                if foot.stepping {
                    foot.stepping = false;
                    foot.pos = v2(foot.to.x, ground_y);
                }
                foot.pos.y = ground_y;
                // Fell too far behind (sudden speed-up): take the next step now.
                if (hip.x - foot.pos.x) * dir > stride * 0.9 {
                    foot.pos.x = hip.x - dir * stride * 0.5;
                }
                let dx = hip.x - foot.pos.x;
                pendulum = pendulum.min((leg * leg - dx * dx).max(0.0).sqrt());
                compress = compress.max((PI * p / duty).sin());
                grounded = true;
            } else {
                // Swing.
                let t = (p - duty) / (1.0 - duty);
                if !foot.stepping {
                    foot.stepping = true;
                    foot.from = foot.pos;
                }
                // Land where the hip will be at touchdown, half a stance ahead.
                let remaining = (1.0 - t) * (1.0 - duty) * stride / speed;
                foot.to = v2(hip.x + vel_x * remaining + dir * half, ground_y);
                foot.t = t;
                foot.dur = ((1.0 - duty) * stride / speed).clamp(0.08, 0.5);
                foot.lift = v2(0.0, -lift_h);
                let arc = (PI * t.powf(0.75)).sin();
                foot.pos = v2(foot.from.x + (foot.to.x - foot.from.x) * smoothstep(t), ground_y - arc * lift_h);
            }
            foot.vel = Vec2::ZERO;
        }
        let walk_off = if pendulum < f32::MAX { (d.stance - pendulum).clamp(-1.5 * s, 5.0 * s) } else { 0.0 };
        let run_off = if grounded { 3.0 * s * compress } else { -2.5 * s };
        (walk_off + (run_off - walk_off) * run) * (1.0 + g.bounce * 0.3)
    }

    /// Squash on landing, proportional to how hard we hit.
    pub fn on_land(&mut self, impact: f32) {
        self.squash.v += (impact * 0.16).min(380.0);
    }

    /// A startled jolt: the body springs up for a moment.
    pub fn jolt(&mut self, amount: f32) {
        self.squash.v -= amount;
    }

    pub fn update(&mut self, inp: &RigInput, d: &Dims, dt: f32) {
        let s = d.s;
        let f = inp.facing;
        let t = inp.time;
        let kind = inp.state.kind();
        let snap = kind != self.kind && self.init;
        let gesture = match inp.state {
            RigState::Stand { gesture, .. } => gesture,
            _ => Gesture::None,
        };
        let dance = gesture == Gesture::Dance;
        self.kind = kind;

        if !self.init {
            for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
                self.feet[i].pos = inp.hip + v2(side * 4.0 * s, d.stance);
                self.hands[i].pos = inp.hip + v2(side * 3.0 * s, -d.torso + d.arm() * 0.9);
            }
            self.init = true;
        }

        self.crouch.update(inp.crouch, 5.0, 0.7, dt);
        self.squash.update(0.0, 4.5, 0.45, dt);
        let plain_sit = matches!(inp.state, RigState::Stand { sit: true, .. }) && !gesture.is_low();
        let sitting = plain_sit || gesture.is_low();
        if plain_sit && !self.was_sitting {
            self.legs_out = (t * 12.9898 + self.seed * 78.233).sin() > 0.0;
        }
        self.was_sitting = plain_sit;
        let legs_out = self.legs_out;
        let pump = 0.5 + 0.5 * (t * 4.5).sin();
        self.sit.update(if sitting { 1.0 } else { 0.0 }, 2.2, 0.9, dt);

        let speed = inp.vel.x.abs();
        let run = (speed / (300.0 * s)).min(1.0);
        let seed = self.seed;
        // How "standing still" we are: drives all the idle noise.
        let idle_k = match inp.state {
            RigState::Stand { .. } => 1.0 - (speed / (40.0 * s)).min(1.0),
            _ => 0.0,
        };
        let fall_k = (inp.vel.y / (900.0 * s)).clamp(0.0, 1.0);
        let rising = inp.vel.y < -30.0 * s;
        let mut hip = inp.hip;
        let lean_target;

        // ---- hips, legs and lean ----
        match inp.state {
            RigState::Stand { ground_y, style: WalkStyle::Crawl, .. } => {
                // On hands and knees: body nearly horizontal, low to the floor.
                let bob = -(self.feet[0].lift_amount() + self.hands[0].lift_amount()) * s;
                hip.y = ground_y - 7.0 * s + bob + self.squash.x.max(0.0) * 0.3;
                let ideal = [v2(hip.x - f * 22.0 * s, ground_y), v2(hip.x - f * 18.0 * s, ground_y)];
                let lead = v2(inp.vel.x * 0.25, 0.0);
                step_pair(&mut self.feet, ideal, lead, 6.0 * s, 0.4, v2(0.0, -2.0 * s), snap, dt);
                lean_target = f * 1.3;
            }
            RigState::Stand { ground_y, gesture: Gesture::Slide, .. } => {
                // Feet-first slide along the ground.
                hip.y = ground_y - 6.0 * s;
                for i in 0..2 {
                    let target = v2(hip.x + f * (20.0 + i as f32 * 5.0) * s, ground_y - i as f32 * 3.0 * s);
                    self.feet[i].follow(target, 10.0, 0.8, dt);
                }
                lean_target = -f * 0.9;
            }
            RigState::Stand { ground_y, style, .. } => {
                let g = gait(style);
                let sitk = self.sit.x.clamp(0.0, 1.0);
                let drop = self.crouch.x * d.stance * 0.38
                    + self.squash.x.max(-4.0 * s)
                    + run * 2.5 * s
                    // Sitting: bottom actually on the ground.
                    + sitk * (d.stance - 2.0 * s)
                    + g.hip * s * (1.0 - sitk)
                    + (if gesture == Gesture::Balance { 2.5 * s } else { 0.0 })
                    // Slumped: sags. Cheering: bounces on the spot.
                    + (if gesture == Gesture::Slump { 2.0 * s } else { 0.0 })
                    - (if gesture == Gesture::Cheer { (t * 9.0).sin().abs() * 2.5 * s } else { 0.0 });
                // Bob from stepping (replaced by the proper gait when walking).
                let step_bob =
                    -(self.feet[0].lift_amount() + self.feet[1].lift_amount()) * (1.0 + run * 1.5) * (1.0 + g.bounce) * s;
                let mut bob = step_bob + (t * 2.1).sin() * 0.4 * s * (1.0 - run);
                if style == WalkStyle::Limp {
                    // Weight drops onto the bad leg (foot 0) while the good one swings.
                    bob += self.feet[1].lift_amount() * 3.5 * s;
                }
                let groove = if dance { -(t * 7.0).sin().abs() * 2.5 * s } else { 0.0 };
                // Fine-tune hip height for the low poses (relative to sitting).
                let low = sitk
                    * match gesture {
                        Gesture::Sleep => -1.0 * s,
                        Gesture::PushUp => -3.0 * s - pump * 5.0 * s,
                        Gesture::Meditate => -(10.0 + (t * 1.6).sin() * 2.0) * s,
                        Gesture::EdgeSit => -1.0 * s,
                        _ => 0.0,
                    };
                hip.y += drop + bob + groove + low;
                hip.x -= f * self.crouch.x * 2.0 * s;
                // Weight slowly shifting from foot to foot (feet re-step when it drifts far).
                hip.x += wobble(t * 0.35, seed) * 2.5 * s * idle_k * (1.0 - sitk);

                if sitk > 0.5 && gesture.is_low() {
                    for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
                        let fi = i as f32;
                        let target = match gesture {
                            Gesture::Sleep => v2(hip.x + f * (25.0 + fi * 2.0) * s, ground_y - s),
                            Gesture::PushUp => v2(hip.x - f * (26.0 + fi * 2.0) * s, ground_y - s),
                            Gesture::Meditate => hip + v2(side * 7.0 * s + f * 2.0 * s, 7.0 * s),
                            _ => {
                                // Legs over the ledge. Every so often it
                                // swings them, in little bursts.
                                let swing = (wobble(t * 0.3, seed + 21.0) * 1.6).clamp(0.0, 1.0);
                                let rate = 2.6 + wobble(t * 0.2, seed + 23.0) * 0.8;
                                let ph = t * rate + fi * PI;
                                v2(
                                    hip.x + f * 7.0 * s + ph.sin() * 3.5 * s * swing + wobble(t * 0.4, seed + fi) * 0.6 * s,
                                    ground_y + 15.0 * s - ph.cos().max(0.0) * 2.5 * s * swing,
                                )
                            }
                        };
                        self.feet[i].follow(target, 6.0, 0.8, dt);
                    }
                } else if sitk > 0.5 {
                    // Sitting on the ground: knees up, or legs stretched out.
                    let reach = if legs_out { 27.0 } else { 13.0 };
                    let ideal = [v2(hip.x + f * reach * s, ground_y), v2(hip.x + f * (reach + 2.0) * s, ground_y)];
                    step_pair(&mut self.feet, ideal, Vec2::ZERO, 3.0 * s, 0.2, v2(0.0, -3.0 * s), snap, dt);
                } else if speed > 15.0 * s && !matches!(gesture, Gesture::TapFoot | Gesture::Kick(_)) {
                    // Walking / running: the proper gait cycle.
                    let off = self.gait_step(hip, inp.vel.x, ground_y, &g, d, dt, run);
                    hip.y += off - step_bob;
                } else {
                    let moving = speed > 15.0 * s;
                    let base = inp.hip.x + inp.vel.x * 0.1;
                    let spread = if moving {
                        0.0
                    } else if dance {
                        4.5 * s * (1.0 + 0.8 * (t * 3.5).sin())
                    } else {
                        // Stance width drifts a little too.
                        4.5 * s * (1.0 + 0.35 * wobble(t * 0.15, seed + 3.0))
                    };
                    let base = base + wobble(t * 0.35, seed) * 2.5 * s * idle_k;
                    let ideal = [v2(base - spread, ground_y), v2(base + spread, ground_y)];
                    let threshold = if moving { (8.0 * s + speed * 0.05) * g.stride } else { 3.0 * s };
                    let dur = ((0.26 - speed / s * 0.000_38) * g.dur).clamp(0.11, 0.4);
                    let lead = v2(inp.vel.x * dur * 0.55, 0.0);
                    let lift = v2(0.0, -(3.5 * s + speed * 0.03) * g.lift);
                    step_pair(&mut self.feet, ideal, lead, threshold, dur, lift, snap, dt);
                    match gesture {
                        Gesture::TapFoot if !self.feet[1].stepping => {
                            self.feet[1].pos.y = ground_y - (t * 9.0).sin().abs() * 2.5 * s;
                        }
                        Gesture::Fume if !self.feet[1].stepping => {
                            // Angry stomping.
                            self.feet[1].pos.y = ground_y - (t * 7.0).sin().max(0.0) * 5.0 * s;
                        }
                        Gesture::Kick(e) => {
                            let leg = d.thigh + d.shin;
                            let target = if e < 0.0 {
                                hip + v2(f * 6.0 * s, 13.0 * s)
                            } else {
                                hip + v2(f * leg * 0.95 * e + f * 6.0 * s * (1.0 - e), 13.0 * s * (1.0 - e) - 2.0 * s * e)
                            };
                            self.feet[1].follow(target, 12.0, 0.7, dt);
                        }
                        _ => {}
                    }
                }
                let walking = (speed / (40.0 * s)).min(1.0);
                let base_lean = (inp.vel.x / s * 0.0008).clamp(-0.32, 0.32)
                    + f * self.crouch.x * 0.3
                    + f * g.lean * walking
                    + wobble(t * 0.25, seed + 5.0) * 0.06 * idle_k
                    // A slight rock forward/back with each step.
                    + (self.phase * TAU * 2.0).sin() * 0.02 * walking;
                lean_target = match gesture {
                    Gesture::Sleep => -f * 1.45 * sitk,
                    Gesture::PushUp => f * 1.3 * sitk,
                    Gesture::Stretch => base_lean - f * 0.12,
                    Gesture::Kick(e) => base_lean - f * 0.3 * e.max(0.0),
                    Gesture::Punch(e) => base_lean + f * e * 0.15,
                    Gesture::Box => base_lean + f * 0.08,
                    Gesture::Dance => base_lean + (t * 3.5).sin() * 0.15,
                    Gesture::Balance => base_lean + (t * 7.0).sin() * 0.18 + wobble(t * 1.7, seed + 31.0) * 0.1,
                    Gesture::Fume => base_lean + f * 0.14,
                    Gesture::Slump => base_lean + f * 0.28,
                    Gesture::Startle => base_lean - f * 0.12,
                    Gesture::Cheer => base_lean - f * 0.06,
                    _ if plain_sit => base_lean + (if legs_out { -f * 0.22 * sitk } else { f * 0.08 * sitk }),
                    _ => base_lean - f * sitk * 0.05,
                };
            }
            RigState::Air => {
                for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
                    let target = if rising {
                        hip + v2(f * 5.0 * s + side * 2.0 * s, 19.0 * s)
                    } else {
                        let flail = (t * 13.0 + i as f32 * 2.0).sin() * 3.0 * s * fall_k;
                        hip + v2(side * 5.0 * s + flail, 26.0 * s)
                    };
                    self.feet[i].follow(target, 7.0, 0.55, dt);
                }
                lean_target = (inp.vel.x / s * 0.0005).clamp(-0.4, 0.4);
            }
            RigState::Climb { wall_x } => {
                let fx = wall_x - f * 2.5 * s;
                let ideal = [v2(fx, hip.y + 18.0 * s), v2(fx, hip.y + 13.0 * s)];
                step_pair(&mut self.feet, ideal, v2(0.0, -5.0 * s), 7.0 * s, 0.16, v2(-f * 4.0 * s, 0.0), snap, dt);
                lean_target = f * 0.06;
            }
            RigState::Mantle { .. } => {
                for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
                    self.feet[i].follow(hip + v2(-f * 3.0 * s + side * 2.0 * s, 22.0 * s), 6.0, 0.6, dt);
                }
                lean_target = f * 0.4;
            }
        }

        // ---- spine and head ----
        self.lean.update(lean_target, 3.0, 0.6, dt);
        let a = self.lean.x;
        let up = v2(a.sin(), -a.cos());
        let stretch = match inp.state {
            RigState::Air => (-inp.vel.y / (2500.0 * s)).clamp(-0.08, 0.12),
            _ => 0.0,
        } - (self.squash.x / d.stance).clamp(0.0, 0.3) * 0.5;
        // Breathing: the chest rises and falls at a slowly varying rate.
        let breath = (t * (2.0 + wobble(t * 0.1, seed + 9.0) * 0.4)).sin() * 0.025 * idle_k;
        let chest = hip + up * (d.torso * (1.0 + stretch + breath));
        let neck = chest + up * d.neck;
        // Head drifts around on its own (looking about, little nods).
        let drift = wobble(t * 0.4, seed + 7.0) * 0.22 * idle_k;
        // Feelings show in the head too: hanging it, cocking it, throwing it back.
        let mood = match gesture {
            Gesture::Slump => f * 0.6,
            Gesture::Scratch => -f * 0.3,
            Gesture::Cheer | Gesture::FistPump => -f * 0.25,
            Gesture::Fume => f * 0.2 + (t * 14.0).sin() * 0.08,
            _ => 0.0,
        };
        self.tilt.update(f * 0.12 + a * 0.4 + drift + mood, 2.5, 0.5, dt);
        let head = neck + v2(self.tilt.x.sin(), -self.tilt.x.cos()) * d.head_r;

        // ---- arms ----
        let arm = d.arm();
        let mut elbow_bend = -f;
        match inp.state {
            RigState::Stand { ground_y, style: WalkStyle::Crawl, .. } => {
                let ideal = [v2(chest.x + f * 3.0 * s, ground_y), v2(chest.x + f * 7.0 * s, ground_y)];
                let lead = v2(inp.vel.x * 0.25, 0.0);
                step_pair(&mut self.hands, ideal, lead, 6.0 * s, 0.35, v2(0.0, -3.0 * s), snap, dt);
            }
            RigState::Stand { ground_y, style, .. } => {
                let g = gait(style);
                let walking = (speed / (40.0 * s)).min(1.0);
                let sitk = self.sit.x.clamp(0.0, 1.0);
                for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
                    let fi = i as f32;
                    let swing = (self.feet[1 - i].pos.x - hip.x) * 0.75 * g.swing;
                    let mut target = chest
                        + v2(
                            swing * (1.0 + run * 0.3) + side * 2.0 * s + f * s,
                            arm * (0.88 - run * 0.3) - swing.abs() * 0.15 + (t * 2.1).sin() * 0.4 * s,
                        );
                    target.x -= f * self.crouch.x * 7.0 * s;
                    // Idle arms sway and fidget slightly, each on its own rhythm.
                    target += v2(
                        wobble(t * 0.5, seed + 11.0 + fi * 3.0) * 2.0 * s,
                        wobble(t * 0.6, seed + 13.0 + fi * 3.0) * 1.5 * s,
                    ) * idle_k;
                    if style == WalkStyle::Sneak && walking > 0.0 {
                        // Hands creeping up in front.
                        let creep = chest + v2(f * (5.0 + fi * 3.0) * s, s + (t * 6.0 + fi * PI).sin() * 1.5 * s);
                        target = target.lerp(creep, walking);
                    }
                    if sitk > 0.3 && plain_sit {
                        // Hands on knees, or planted behind it on the ground.
                        let rest = if legs_out {
                            v2(hip.x - f * (6.0 + fi * 2.0) * s, ground_y)
                        } else {
                            self.pose.knees[i] + v2(0.0, -s)
                        };
                        target = target.lerp(rest, sitk);
                    }
                    let mut freq = 5.5;
                    match gesture {
                        Gesture::Wave if i == 1 => {
                            target = chest + v2(f * 7.0 * s + (t * 13.0).sin() * 5.0 * s, -17.0 * s);
                        }
                        Gesture::Dance => {
                            let ph = t * 7.0 + fi * PI;
                            target = chest + v2(side * 9.0 * s + ph.sin() * 3.0 * s, -13.0 * s + ph.sin() * 6.0 * s);
                        }
                        Gesture::Punch(e) if i == 1 => {
                            target = if e < 0.0 {
                                chest + v2(-f * 9.0 * s * -e, 3.0 * s)
                            } else {
                                chest + v2(f * (arm * 0.95 * e + 2.0 * s * (1.0 - e)), -2.0 * s)
                            };
                            freq = 12.0;
                        }
                        Gesture::Punch(_) => {
                            // Guard hand up.
                            target = chest + v2(f * 6.0 * s, 1.0 * s);
                            freq = 8.0;
                        }
                        Gesture::Kick(_) => {
                            // Arms out for balance.
                            target = chest + v2(side * 10.0 * s, -4.0 * s);
                            freq = 8.0;
                        }
                        Gesture::Stretch => {
                            target = chest
                                + v2(
                                    side * 4.0 * s + f * 2.0 * s + wobble(t * 1.3, seed + fi * 5.0) * 2.0 * s,
                                    -arm * (0.88 + 0.06 * wobble(t * 0.9, seed + fi)),
                                );
                            freq = 3.0;
                        }
                        Gesture::Scratch if i == 1 => {
                            let rate = 20.0 + wobble(t * 0.7, seed + 17.0) * 6.0;
                            target = head
                                + v2(
                                    f * 2.0 * s + (t * rate).sin() * 1.5 * s,
                                    -d.head_r * (0.6 + 0.3 * wobble(t * 0.5, seed + 19.0)),
                                );
                            freq = 9.0;
                        }
                        Gesture::TapFoot => {
                            // Arms crossed.
                            target = chest + v2(f * (4.0 + fi * 2.0) * s, 7.0 * s);
                            freq = 7.0;
                        }
                        Gesture::Box => {
                            let e = (t * 9.0 + fi * PI).sin().max(0.0);
                            target = chest + v2(f * (4.0 * s + arm * 0.75 * e), -3.0 * s);
                            freq = 14.0;
                        }
                        Gesture::Meditate => {
                            target = chest + v2(side * 12.0 * s + f * s, 9.0 * s);
                            freq = 3.0;
                        }
                        Gesture::PushUp => {
                            target = v2(chest.x + f * fi * 2.0 * s, ground_y);
                            freq = 10.0;
                        }
                        Gesture::Sleep => {
                            target = chest + v2(f * (3.0 + fi * 2.0) * s, 2.0 * s);
                            freq = 3.0;
                        }
                        Gesture::EdgeSit => {
                            target = v2(hip.x - f * (3.0 + fi * 3.0) * s, ground_y);
                            freq = 6.0;
                        }
                        Gesture::Balance => {
                            // Arms out wide, windmilling.
                            let ph = t * 9.0 + fi * PI;
                            target = chest + v2(side * arm * 0.75 + ph.cos() * 5.0 * s, -4.0 * s + ph.sin() * 5.0 * s);
                            freq = 9.0;
                        }
                        Gesture::Spin => {
                            target = chest + v2(side * arm * 0.85, -3.0 * s);
                            freq = 12.0;
                        }
                        Gesture::Slide => {
                            // One hand dragging behind, one thrown up.
                            target = if i == 0 { v2(hip.x - f * 6.0 * s, ground_y) } else { chest + v2(f * 4.0 * s, -18.0 * s) };
                            freq = 10.0;
                        }
                        Gesture::HighFive(e) if i == 1 => {
                            target = chest + v2(f * (5.0 * s + arm * 0.45 * e), -(6.0 * s + arm * 0.6 * e));
                            freq = 11.0;
                        }
                        Gesture::Startle => {
                            // Arms flung up and out.
                            target = chest + v2(side * 10.0 * s + f * 2.0 * s, -14.0 * s);
                            freq = 14.0;
                        }
                        Gesture::Cheer => {
                            // Both arms up in a V, pumping.
                            let pump = (t * 9.0 + fi * 0.4).sin().abs();
                            target = chest + v2(side * (7.0 + pump * 2.0) * s, -arm * (0.75 + pump * 0.12));
                            freq = 10.0;
                        }
                        Gesture::Fume => {
                            // Fists clenched at its sides, shaking.
                            target = chest + v2(side * 5.0 * s + f * 3.0 * s, arm * 0.6 + (t * 20.0 + fi).sin() * s);
                            freq = 12.0;
                        }
                        Gesture::Slump => {
                            // Arms hanging limp.
                            target = chest + v2(side * 1.5 * s + f * 3.0 * s, arm * 0.97);
                            freq = 3.5;
                        }
                        Gesture::FistPump if i == 1 => {
                            target = chest + v2(f * 3.0 * s, -arm * 0.9 + (t * 12.0).sin().abs() * 4.0 * s);
                            freq = 12.0;
                        }
                        _ if style == WalkStyle::Panic => {
                            // Arms flailing over its head.
                            target = chest
                                + v2(side * 8.0 * s + (t * 20.0 + fi).sin() * 4.0 * s, -16.0 * s + (t * 17.0 + fi).cos() * 4.0 * s);
                            freq = 10.0;
                        }
                        _ => {}
                    }
                    self.hands[i].follow(target, freq, 0.6, dt);
                }
            }
            RigState::Air => {
                for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
                    let fi = i as f32;
                    let target = if rising {
                        chest + v2(f * 9.0 * s + side * 3.0 * s, -11.0 * s)
                    } else {
                        chest
                            + v2(
                                side * 11.0 * s + (t * 15.0 + fi * 1.7).sin() * 3.0 * s * fall_k,
                                -9.0 * s + (t * 11.0 + fi).cos() * 4.0 * s * fall_k,
                            )
                    };
                    self.hands[i].follow(target, 6.0, 0.5, dt);
                }
            }
            RigState::Climb { wall_x } => {
                let hx = wall_x - f * 1.5 * s;
                let ideal = [v2(hx, chest.y - 10.0 * s), v2(hx, chest.y - 16.0 * s)];
                step_pair(&mut self.hands, ideal, v2(0.0, -6.0 * s), 7.0 * s, 0.15, v2(-f * 4.0 * s, 0.0), snap, dt);
                elbow_bend = f;
            }
            RigState::Mantle { wall_x, top_y } => {
                for i in 0..2 {
                    let target = v2(wall_x + f * (3.0 + i as f32 * 5.0) * s, top_y);
                    self.hands[i].follow(target, 9.0, 0.8, dt);
                }
            }
        }

        // ---- solve joints ----
        let (k0, f0) = ik(hip, self.feet[0].pos, d.thigh, d.shin, f);
        let (k1, f1) = ik(hip, self.feet[1].pos, d.thigh, d.shin, f);
        let (e0, h0) = ik(chest, self.hands[0].pos, d.upper_arm, d.forearm, elbow_bend);
        let (e1, h1) = ik(chest, self.hands[1].pos, d.upper_arm, d.forearm, elbow_bend);
        self.pose = Pose {
            hip,
            chest,
            neck,
            head,
            knees: [k0, k1],
            feet: [f0, f1],
            elbows: [e0, e1],
            hands: [h0, h1],
        };
    }
}
