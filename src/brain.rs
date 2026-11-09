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
    /// Jump so the feet land at `target`.
    Jump { target: Vec2 },
    Climb { win: WinId, side: Side },
    /// Switch between walking in front of and behind windows.
    ToggleLayer,
}

/// What the brain gets to see when making a decision.
pub struct Perception {
    pub hip: Vec2,
    pub foot: Vec2,
    pub support: Support,
    pub scale: f32,
    pub max_jump: f32,
    pub cursor: Vec2,
}

pub enum BrainEvent {
    Grabbed,
    Thrown,
    Landed(f32),
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
    Jump,
    Climb,
    Drop,
    Layer,
    Follow,
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
}

impl UtilityBrain {
    pub fn new(personality: Personality) -> Self {
        UtilityBrain {
            personality,
            needs: Needs { boredom: 0.3, energy: 0.9 },
            current: Kind::Idle,
            history: Vec::new(),
        }
    }

    fn commit(&mut self, kind: Kind) {
        let n = &mut self.needs;
        match kind {
            Kind::Jump | Kind::Climb => {
                n.energy -= 0.1;
                n.boredom -= 0.35;
            }
            Kind::Run => {
                n.energy -= 0.06;
                n.boredom -= 0.25;
            }
            Kind::Drop | Kind::Layer | Kind::Wave | Kind::Follow => n.boredom -= 0.2,
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
        }
    }

    fn decide(&mut self, p: &Perception, world: &World, rng: &mut Rng) -> Intent {
        let s = p.scale;
        let Some(surf) = world.surface(p.support) else {
            return Intent::Idle { secs: 0.5 };
        };
        let pers = self.personality;
        let (b, e) = (self.needs.boredom, self.needs.energy);
        let mut options: Vec<(Kind, Intent, f32)> = Vec::new();

        options.push((Kind::Idle, Intent::Idle { secs: rng.range(0.8, 2.5) }, 0.35 + pers.laziness * 0.3 - b * 0.35));
        options.push((
            Kind::Sit,
            Intent::Sit { secs: rng.range(3.0, 9.0) },
            (1.0 - e) * 0.9 + pers.laziness * 0.2 - b * 0.25,
        ));

        let margin = 14.0 * s;
        if surf.x1 - surf.x0 > margin * 3.0 {
            let x = rng.range(surf.x0 + margin, surf.x1 - margin);
            options.push((Kind::Walk, Intent::Walk { x, speed: 85.0 * s }, 0.45 + pers.curiosity * 0.15));
            let far = if p.hip.x - surf.x0 > surf.x1 - p.hip.x { surf.x0 + margin } else { surf.x1 - margin };
            options.push((
                Kind::Run,
                Intent::Walk { x: far, speed: 330.0 * s },
                (b * 0.6 + pers.playfulness * 0.3) * e.max(0.15),
            ));
        }

        // Best jump to another surface.
        let mut best_jump: Option<(Intent, f32)> = None;
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
            let score = (0.3 + b * 0.5 + pers.playfulness * 0.3 + (rise.max(0.0) / p.max_jump) * pers.curiosity * 0.2)
                * e.max(0.2)
                - dx / (3000.0 * s)
                + rng.f32() * 0.1;
            if best_jump.map_or(true, |(_, bs)| score > bs) {
                best_jump = Some((Intent::Jump { target: v2(x, t.y) }, score));
            }
        }
        if let Some((i, sc)) = best_jump {
            options.push((Kind::Jump, i, sc));
        }

        // Best window side to climb.
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
                let score = (0.3 + pers.curiosity * 0.4 + b * 0.35) * e.max(0.2)
                    - (stand - p.hip.x).abs() / (2500.0 * s)
                    + rng.f32() * 0.1;
                if best_climb.map_or(true, |(_, bs)| score > bs) {
                    best_climb = Some((Intent::Climb { win: w.id, side }, score));
                }
            }
        }
        if let Some((i, sc)) = best_climb {
            options.push((Kind::Climb, i, sc));
        }

        // Step off the edge of the window we're on.
        if let Support::Window(_) = p.support {
            let x = if rng.chance(0.5) { surf.x0 - 12.0 * s } else { surf.x1 + 12.0 * s };
            options.push((Kind::Drop, Intent::Walk { x, speed: 110.0 * s }, 0.12 + pers.mischief * 0.35 + b * 0.2));
        }

        let near = (p.cursor - p.hip).len() < 260.0 * s;
        options.push((
            Kind::Wave,
            Intent::Wave { secs: rng.range(1.2, 2.2) },
            if near { 0.2 + pers.playfulness * 0.4 } else { 0.06 },
        ));

        if (p.cursor.y - surf.y).abs() < 320.0 * s
            && p.cursor.x > surf.x0
            && p.cursor.x < surf.x1
            && (p.cursor.x - p.hip.x).abs() > 40.0 * s
        {
            options.push((
                Kind::Follow,
                Intent::Walk { x: p.cursor.x, speed: 150.0 * s },
                pers.curiosity * 0.35 + b * 0.1,
            ));
        }

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
