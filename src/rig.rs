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
}

impl Gesture {
    /// Poses that bring the hips down to the ground.
    fn is_low(self) -> bool {
        matches!(self, Gesture::Meditate | Gesture::PushUp | Gesture::Sleep | Gesture::EdgeSit)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum RigState {
    Stand { ground_y: f32, sit: bool, gesture: Gesture },
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

    /// Squash on landing, proportional to how hard we hit.
    pub fn on_land(&mut self, impact: f32) {
        self.squash.v += (impact * 0.16).min(380.0);
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
        let sitting = matches!(inp.state, RigState::Stand { sit: true, .. }) || gesture.is_low();
        let pump = 0.5 + 0.5 * (t * 4.5).sin();
        self.sit.update(if sitting { 1.0 } else { 0.0 }, 2.2, 0.9, dt);

        let speed = inp.vel.x.abs();
        let run = (speed / (300.0 * s)).min(1.0);
        let fall_k = (inp.vel.y / (900.0 * s)).clamp(0.0, 1.0);
        let rising = inp.vel.y < -30.0 * s;
        let mut hip = inp.hip;
        let lean_target;

        // ---- hips, legs and lean ----
        match inp.state {
            RigState::Stand { ground_y, .. } => {
                let sitk = self.sit.x.clamp(0.0, 1.0);
                let drop = self.crouch.x * d.stance * 0.38
                    + self.squash.x.max(-4.0 * s)
                    + run * 2.5 * s
                    + sitk * (d.stance - 7.0 * s);
                let bob = -(self.feet[0].lift_amount() + self.feet[1].lift_amount())
                    * (1.0 + run * 1.5)
                    * s
                    + (t * 2.1).sin() * 0.4 * s * (1.0 - run);
                let groove = if dance { -(t * 7.0).sin().abs() * 2.5 * s } else { 0.0 };
                // Fine-tune hip height for the low poses (relative to sitting).
                let low = sitk
                    * match gesture {
                        Gesture::Sleep => 4.0 * s,
                        Gesture::PushUp => 2.0 * s - pump * 5.0 * s,
                        Gesture::Meditate => -(5.0 + (t * 1.6).sin() * 2.0) * s,
                        Gesture::EdgeSit => 4.0 * s,
                        _ => 0.0,
                    };
                hip.y += drop + bob + groove + low;
                hip.x -= f * self.crouch.x * 2.0 * s;

                if sitk > 0.5 && gesture.is_low() {
                    for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
                        let fi = i as f32;
                        let target = match gesture {
                            Gesture::Sleep => v2(hip.x + f * (25.0 + fi * 2.0) * s, ground_y - s),
                            Gesture::PushUp => v2(hip.x - f * (26.0 + fi * 2.0) * s, ground_y - s),
                            Gesture::Meditate => hip + v2(side * 7.0 * s + f * 2.0 * s, 7.0 * s),
                            _ => v2(
                                hip.x + f * 7.0 * s + (t * 2.5 + fi * PI).sin() * 3.0 * s,
                                ground_y + 15.0 * s,
                            ),
                        };
                        self.feet[i].follow(target, 6.0, 0.8, dt);
                    }
                } else if sitk > 0.5 {
                    let ideal = [v2(hip.x + f * 13.0 * s, ground_y), v2(hip.x + f * 16.0 * s, ground_y)];
                    step_pair(&mut self.feet, ideal, Vec2::ZERO, 3.0 * s, 0.2, v2(0.0, -3.0 * s), snap, dt);
                } else {
                    let moving = speed > 15.0 * s;
                    let base = inp.hip.x + inp.vel.x * 0.1;
                    let spread = if moving {
                        0.0
                    } else if dance {
                        4.5 * s * (1.0 + 0.8 * (t * 3.5).sin())
                    } else {
                        4.5 * s
                    };
                    let ideal = [v2(base - spread, ground_y), v2(base + spread, ground_y)];
                    let threshold = if moving { 8.0 * s + speed * 0.05 } else { 3.0 * s };
                    let dur = (0.26 - speed / s * 0.000_38).clamp(0.11, 0.26);
                    let lead = v2(inp.vel.x * dur * 0.55, 0.0);
                    let lift = v2(0.0, -(3.5 * s + speed * 0.03));
                    step_pair(&mut self.feet, ideal, lead, threshold, dur, lift, snap, dt);
                    match gesture {
                        Gesture::TapFoot if !self.feet[1].stepping => {
                            self.feet[1].pos.y = ground_y - (t * 9.0).sin().abs() * 2.5 * s;
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
                let base_lean = (inp.vel.x / s * 0.0008).clamp(-0.32, 0.32) + f * self.crouch.x * 0.3;
                lean_target = match gesture {
                    Gesture::Sleep => -f * 1.45 * sitk,
                    Gesture::PushUp => f * 1.3 * sitk,
                    Gesture::Stretch => base_lean - f * 0.12,
                    Gesture::Kick(e) => base_lean - f * 0.3 * e.max(0.0),
                    Gesture::Punch(e) => base_lean + f * e * 0.15,
                    Gesture::Box => base_lean + f * 0.08,
                    Gesture::Dance => base_lean + (t * 3.5).sin() * 0.15,
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
        let chest = hip + up * (d.torso * (1.0 + stretch));
        let neck = chest + up * d.neck;
        self.tilt.update(f * 0.12 + a * 0.4, 2.5, 0.5, dt);
        let head = neck + v2(self.tilt.x.sin(), -self.tilt.x.cos()) * d.head_r;

        // ---- arms ----
        let arm = d.arm();
        let mut elbow_bend = -f;
        match inp.state {
            RigState::Stand { ground_y, .. } => {
                let sitk = self.sit.x.clamp(0.0, 1.0);
                for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
                    let fi = i as f32;
                    let swing = (self.feet[1 - i].pos.x - hip.x) * 0.75;
                    let mut target = chest
                        + v2(
                            swing * (1.0 + run * 0.3) + side * 2.0 * s + f * s,
                            arm * (0.88 - run * 0.3) - swing.abs() * 0.15 + (t * 2.1).sin() * 0.4 * s,
                        );
                    target.x -= f * self.crouch.x * 7.0 * s;
                    if sitk > 0.3 {
                        target = target.lerp(self.pose.knees[i] + v2(0.0, -s), sitk);
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
                            target = chest + v2(side * 4.0 * s + f * 2.0 * s + (t * 2.0 + fi).sin() * s, -arm * 0.92);
                            freq = 3.0;
                        }
                        Gesture::Scratch if i == 1 => {
                            target = head + v2(f * 2.0 * s + (t * 22.0).sin() * 1.5 * s, -d.head_r * 0.6);
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
