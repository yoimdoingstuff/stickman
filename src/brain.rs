//! Decides what the stickman does next. `Brain` is a trait so the simple
//! utility AI here can later be swapped for a trained model (offline, loaded
//! from a weights file) without touching the body or animation code.

use crate::math::*;
use crate::rng::Rng;
use crate::world::*;

#[derive(Clone, Copy, Debug)]
pub enum Intent {
    Idle { secs: f32 },
    Walk { x: f32, speed: f32 },
    Sit { secs: f32 },
    Wave { secs: f32 },
    Dance { secs: f32 },
    /// Jump so the feet land at `target` on `support`.
    Jump { target: Vec2, support: Support },
    Flip,
    Climb { win: WinId, side: Side },
    /// Switch between walking in front of and behind windows.
    ToggleLayer,
    /// Jump up, grab the mouse cursor and hang off it for a while.
    GrabCursor { secs: f32 },
    Punch { target: u32, dir: f32 },
}

/// Another stickman (from another stickman.exe).
#[derive(Clone, Copy, Debug)]
pub struct PeerView {
    pub id: u32,
    pub hip: Vec2,
    pub ragdoll: bool,
}

/// What the brain gets to see when making a decision.
pub struct Perception {
    pub hip: Vec2,
    pub foot: Vec2,
    pub support: Support,
    pub scale: f32,
    pub max_jump: f32,
    /// Feet-to-hands height when reaching up.
    pub reach: f32,
    pub cursor: Vec2,
    pub allow_cursor: bool,
    pub peers: Vec<PeerView>,
}

pub enum BrainEvent {
    Grabbed,
    Thrown,
    Landed(f32),
    /// Got hit by another stickman.
    Hit { from: u32 },
}

pub trait Brain {
    fn tick(&mut self, dt: f32);
    fn decide(&mut self, p: &Perception, world: &World, rng: &mut Rng) -> Intent;
    fn notify(&mut self, e: BrainEvent);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Idle,
    Walk,
    Run,
    Sit,
    Wave,
    Dance,
    Jump,
    Enter,
    Flip,
    Climb,
    Drop,
    Layer,
    Follow,
    Cursor,
    Approach,
    Punch,
}

/// Fixed traits, rolled once per stickman. All 0..1.
#[derive(Clone, Copy, Debug)]
pub struct Personality {
    pub curiosity: f32,
    pub laziness: f32,
    pub mischief: f32,
    pub playfulness: f32,
}

impl Personality {
    pub fn random(rng: &mut Rng) -> Self {
        Personality {
            curiosity: rng.range(0.2, 0.9),
            laziness: rng.range(0.1, 0.7),
            mischief: rng.range(0.2, 0.9),
            playfulness: rng.range(0.3, 0.9),
        }
    }
}

/// Needs that drift over time. All 0..1.
#[derive(Clone, Copy, Debug)]
pub struct Needs {
    pub boredom: f32,
    pub energy: f32,
}

/// Scores every possible action from personality + needs, penalizes things
/// it did recently (so it gets bored of repeating itself), adds some noise
/// and picks the best.
pub struct UtilityBrain {
    pub personality: Personality,
    pub needs: Needs,
    current: Kind,
    history: Vec<Kind>,
    /// Who hit us last, and how annoyed we still are (0..1).
    grudge: Option<(u32, f32)>,
}

impl UtilityBrain {
    pub fn new(personality: Personality) -> Self {
        UtilityBrain {
            personality,
            needs: Needs { boredom: 0.3, energy: 0.9 },
            current: Kind::Idle,
            history: Vec::new(),
            grudge: None,
        }
    }

    fn grudge_against(&self, id: u32) -> f32 {
        match self.grudge {
            Some((g, amount)) if g == id => amount,
            _ => 0.0,
        }
    }

    fn commit(&mut self, kind: Kind) {
        let n = &mut self.needs;
        match kind {
            Kind::Jump | Kind::Climb | Kind::Flip | Kind::Enter | Kind::Cursor => {
                n.energy -= 0.1;
                n.boredom -= 0.35;
            }
            Kind::Run | Kind::Dance => {
                n.energy -= 0.06;
                n.boredom -= 0.25;
            }
            Kind::Punch => {
                n.boredom -= 0.3;
                if let Some((_, g)) = &mut self.grudge {
                    *g -= 0.35;
                }
            }
            Kind::Drop | Kind::Layer | Kind::Wave | Kind::Follow | Kind::Approach => n.boredom -= 0.2,
            Kind::Walk => n.boredom -= 0.08,
            Kind::Idle | Kind::Sit => {}
        }
        n.boredom = n.boredom.clamp(0.0, 1.0);
        n.energy = n.energy.clamp(0.0, 1.0);
        self.current = kind;
        self.history.push(kind);
        if self.history.len() > 6 {
            self.history.remove(0);
        }
    }
}

impl Brain for UtilityBrain {
    fn tick(&mut self, dt: f32) {
        let n = &mut self.needs;
        match self.current {
            Kind::Sit => {
                n.energy += 0.05 * dt;
                n.boredom += 0.015 * dt;
            }
            Kind::Idle => {
                n.energy += 0.01 * dt;
                n.boredom += 0.04 * dt;
            }
            _ => n.boredom += 0.01 * dt,
        }
        n.boredom = n.boredom.clamp(0.0, 1.0);
        n.energy = n.energy.clamp(0.0, 1.0);
        if let Some((_, g)) = &mut self.grudge {
            *g -= 0.02 * dt;
        }
        if matches!(self.grudge, Some((_, g)) if g <= 0.0) {
            self.grudge = None;
        }
    }

    fn notify(&mut self, e: BrainEvent) {
        let n = &mut self.needs;
        match e {
            BrainEvent::Grabbed => {
                n.boredom = (n.boredom - 0.3).max(0.0);
                self.current = Kind::Idle;
            }
            BrainEvent::Thrown => n.boredom = (n.boredom - 0.2).max(0.0),
            BrainEvent::Landed(impact) => {
                if impact > 1500.0 {
                    n.energy = (n.energy - 0.05).max(0.0);
                }
            }
            BrainEvent::Hit { from } => {
                let g = (self.grudge_against(from) + 0.5 + self.personality.mischief * 0.3).min(1.0);
                self.grudge = Some((from, g));
                self.current = Kind::Idle;
            }
        }
    }

    fn decide(&mut self, p: &Perception, world: &World, rng: &mut Rng) -> Intent {
        let s = p.scale;
        let Some(surf) = world.surface(p.support) else {
            return Intent::Idle { secs: 0.5 };
        };
        let pers = self.personality;
        let (b, e) = (self.needs.boredom, self.needs.energy);
        let ef = e.max(0.2);
        let mut options: Vec<(Kind, Intent, f32)> = Vec::new();
        let inside = matches!(p.support, Support::Inside(_));

        options.push((Kind::Idle, Intent::Idle { secs: rng.range(0.8, 2.5) }, 0.35 + pers.laziness * 0.3 - b * 0.35));
        options.push((
            Kind::Sit,
            Intent::Sit { secs: rng.range(3.0, 9.0) },
            (1.0 - e) * 0.9 + pers.laziness * 0.2 - b * 0.25,
        ));
        options.push((
            Kind::Dance,
            Intent::Dance { secs: rng.range(2.0, 4.5) },
            (0.08 + pers.playfulness * 0.3 + b * 0.15) * ef,
        ));
        options.push((Kind::Flip, Intent::Flip, (0.1 + pers.playfulness * 0.35) * ef));

        let margin = 14.0 * s;
        if surf.x1 - surf.x0 > margin * 3.0 {
            let x = rng.range(surf.x0 + margin, surf.x1 - margin);
            options.push((Kind::Walk, Intent::Walk { x, speed: 85.0 * s }, 0.45 + pers.curiosity * 0.15));
            let far = if p.hip.x - surf.x0 > surf.x1 - p.hip.x { surf.x0 + margin } else { surf.x1 - margin };
            options.push((Kind::Run, Intent::Walk { x: far, speed: 330.0 * s }, (b * 0.6 + pers.playfulness * 0.3) * ef));
        }

        // Best jump to another surface, and best jump *into* a window.
        let mut best_jump: Option<(Intent, f32)> = None;
        let mut best_enter: Option<(Intent, f32)> = None;
        let foreground = world.windows.first().map(|w| w.id);
        for t in world.surfaces() {
            if t.support == p.support || t.x1 - t.x0 < 40.0 * s {
                continue;
            }
            let x = rng.range(t.x0 + 15.0 * s, t.x1 - 15.0 * s);
            let rise = p.foot.y - t.y;
            let dx = (x - p.hip.x).abs();
            if rise > p.max_jump || dx > 650.0 * s || (rise.abs() < 10.0 * s && dx < 30.0 * s) {
                continue;
            }
            let intent = Intent::Jump { target: v2(x, t.y), support: t.support };
            if let Support::Inside(id) = t.support {
                let fg = if foreground == Some(id) { 0.15 } else { 0.0 };
                let score = (0.2 + pers.curiosity * 0.4 + b * 0.3 + fg) * ef - dx / (3000.0 * s) + rng.f32() * 0.1;
                if best_enter.map_or(true, |(_, bs)| score > bs) {
                    best_enter = Some((intent, score));
                }
            } else {
                // Inside a window, leaving is more tempting the more bored we get.
                let leave = if inside { b * 0.3 } else { 0.0 };
                let score = (0.3 + b * 0.5 + pers.playfulness * 0.3 + (rise.max(0.0) / p.max_jump) * pers.curiosity * 0.2 + leave) * ef
                    - dx / (3000.0 * s)
                    + rng.f32() * 0.1;
                if best_jump.map_or(true, |(_, bs)| score > bs) {
                    best_jump = Some((intent, score));
                }
            }
        }
        if let Some((i, sc)) = best_jump {
            options.push((Kind::Jump, i, sc));
        }
        if let Some((i, sc)) = best_enter {
            options.push((Kind::Enter, i, sc));
        }

        // Best window side to climb (from outside windows only).
        if !inside {
            let mut best_climb: Option<(Intent, f32)> = None;
            for w in &world.windows {
                if Support::Window(w.id) == p.support || world.surface(Support::Window(w.id)).is_none() {
                    continue;
                }
                if w.rect.t > p.foot.y - 50.0 * s || w.rect.b < p.foot.y - (p.max_jump + 40.0 * s) {
                    continue;
                }
                for side in [Side::Left, Side::Right] {
                    let Some(wx) = world.wall_x(w.id, side) else { continue };
                    let stand = wx + side.outward() * 9.0 * s;
                    if stand < surf.x0 + 4.0 * s || stand > surf.x1 - 4.0 * s {
                        continue;
                    }
                    let score = (0.3 + pers.curiosity * 0.4 + b * 0.35) * ef - (stand - p.hip.x).abs() / (2500.0 * s)
                        + rng.f32() * 0.1;
                    if best_climb.map_or(true, |(_, bs)| score > bs) {
                        best_climb = Some((Intent::Climb { win: w.id, side }, score));
                    }
                }
            }
            if let Some((i, sc)) = best_climb {
                options.push((Kind::Climb, i, sc));
            }
        }

        // Step off the edge of the window we're on.
        if let Support::Window(_) = p.support {
            let x = if rng.chance(0.5) { surf.x0 - 12.0 * s } else { surf.x1 + 12.0 * s };
            options.push((Kind::Drop, Intent::Walk { x, speed: 110.0 * s }, 0.12 + pers.mischief * 0.35 + b * 0.2));
        }

        // The cursor: wave at it, follow it, or jump up and grab it.
        let to_cursor = p.cursor - p.hip;
        let mut wave = if to_cursor.len() < 260.0 * s { 0.2 + pers.playfulness * 0.4 } else { 0.06 };
        if (p.cursor.y - surf.y).abs() < 320.0 * s
            && p.cursor.x > surf.x0
            && p.cursor.x < surf.x1
            && to_cursor.x.abs() > 40.0 * s
        {
            options.push((
                Kind::Follow,
                Intent::Walk { x: p.cursor.x, speed: 150.0 * s },
                pers.curiosity * 0.35 + b * 0.1,
            ));
        }
        let above = p.foot.y - p.cursor.y;
        if p.allow_cursor
            && above > p.reach + 10.0 * s
            && above < p.max_jump + p.reach
            && to_cursor.x.abs() < 250.0 * s
        {
            options.push((
                Kind::Cursor,
                Intent::GrabCursor { secs: rng.range(2.5, 7.0) },
                (0.2 + pers.playfulness * 0.4 + pers.mischief * 0.2) * ef,
            ));
        }

        // Other stickmen on the same level.
        for peer in &p.peers {
            if peer.ragdoll || (peer.hip.y - p.hip.y).abs() > 8.0 * s {
                continue;
            }
            let dx = peer.hip.x - p.hip.x;
            let dir = if dx >= 0.0 { 1.0 } else { -1.0 };
            let grudge = self.grudge_against(peer.id);
            if dx.abs() < 400.0 * s {
                wave = wave.max(0.15 + pers.playfulness * 0.2);
            }
            if dx.abs() < 26.0 * s {
                options.push((
                    Kind::Punch,
                    Intent::Punch { target: peer.id, dir },
                    0.1 + pers.mischief * 0.5 + grudge * 0.8,
                ));
            } else if dx.abs() < 600.0 * s && peer.hip.x > surf.x0 && peer.hip.x < surf.x1 {
                let speed = if grudge > 0.2 { 300.0 } else { 120.0 } * s;
                options.push((
                    Kind::Approach,
                    Intent::Walk { x: peer.hip.x - dir * 18.0 * s, speed },
                    0.15 + pers.curiosity * 0.25 + grudge * 0.6,
                ));
            }
        }
        options.push((Kind::Wave, Intent::Wave { secs: rng.range(1.2, 2.2) }, wave));

        if p.support == Support::Floor && !world.windows.is_empty() {
            options.push((Kind::Layer, Intent::ToggleLayer, 0.08 + pers.mischief * 0.25));
        }

        let mut best = (Kind::Idle, Intent::Idle { secs: 1.0 }, f32::MIN);
        for (kind, intent, score) in options {
            let repeats = self.history.iter().filter(|h| **h == kind).count() as i32;
            let score = score * 0.6f32.powi(repeats) + rng.f32() * 0.3;
            if score > best.2 {
                best = (kind, intent, score);
            }
        }
        self.commit(best.0);
        best.1
    }
}
