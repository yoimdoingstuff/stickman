//! Decides what the stickman does next.
//!
//! Two layers work together:
//! * **Instinct**: hand-written scores from personality and needs (boredom,
//!   energy), so it behaves sensibly from the very first second.
//! * **Learning**: a tiny neural network (see `learn.rs`) that predicts how
//!   much it will enjoy each option *in this situation*, learned from what
//!   actually happened every time it did something: did it get hurt, have
//!   fun, make a friend, did you pay attention, did you praise or scold it.
//!   The more it has experienced, the more it trusts what it learned over
//!   its instincts.
//!
//! On top of that it keeps relationships with the other stickmen (friends
//! and enemies, built up from high-fives, fights and who stood up for whom).
//! It never says anything: what it feels comes out as body language
//! (a startled jolt, a puzzled head-scratch, a happy cheer, an angry stomp).

use crate::games::{GameEvent, Outcome};
use crate::learn::{self, Experience, Net, CTX, TAGS};
use crate::math::*;
use crate::rig::{Gesture, WalkStyle};
use crate::rng::Rng;
use crate::world::*;

#[derive(Clone, Copy, Debug)]
pub enum Intent {
    /// Stand around, optionally fidgeting.
    Idle { secs: f32, fidget: Gesture },
    Walk { x: f32, speed: f32, style: WalkStyle },
    Sit { secs: f32 },
    /// Hold a gesture: wave, dance, sleep, push-ups, meditate, shadowbox...
    Pose { secs: f32, gesture: Gesture },
    Kick { target: Option<u32>, dir: f32 },
    /// Sit on the ledge at `edge`, facing `dir` (toward the drop).
    EdgeSit { secs: f32, edge: f32, dir: f32 },
    Cartwheel,
    /// Jump so the feet land at `target` on `support`.
    Jump { target: Vec2, support: Support },
    Flip,
    /// Climb a window's side; `inner` = from inside the window.
    Climb { win: WinId, side: Side, inner: bool },
    /// Dive into the taskbar and pop out at another window.
    Portal { target: Support },
    /// Switch between walking in front of and behind windows.
    ToggleLayer,
    /// Jump up, grab the mouse cursor and hang off it for a while.
    GrabCursor { secs: f32 },
    /// Jump up, grab the bottom of a desktop icon and swing on it.
    Swing { at: Vec2, secs: f32 },
    Punch { target: u32, dir: f32 },
    /// Tap a (sleeping) stickman to wake it up.
    Poke { target: u32, dir: f32 },
    /// Hand up for a high-five with a friend.
    HighFive { target: u32, dir: f32 },
    /// Random silliness.
    Trip,
    Tantrum,
    PlayDead { secs: f32 },
    /// Run off this side of the screen and come back in the other.
    Wrap { dir: f32 },
    Spin { secs: f32 },
    Slide,
    HighLeap,
}

/// Another stickman.
#[derive(Clone, Copy, Debug)]
pub struct PeerView {
    pub id: u32,
    pub hip: Vec2,
    pub ragdoll: bool,
    pub sleeping: bool,
    /// Bottom of its body (where it's standing or lying).
    pub ground: f32,
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
    /// 0..1, hidden health.
    pub health: f32,
    pub layer: Layer,
    /// Current gravity, px/s².
    pub gravity: f32,
    /// A game you're playing right now (it's the window in front).
    pub focus_game: Option<WinId>,
    pub allow_cursor: bool,
    /// May kick desktop icons around.
    pub move_icons: bool,
    pub peers: Vec<PeerView>,
    /// 0..1: how fast what it's standing on is moving.
    pub shaky: f32,
    /// 0..1: how high up the screen it is (0 = the floor).
    pub height: f32,
    /// 0..1: how much you've been paying attention to it lately.
    pub attention: f32,
}

pub enum BrainEvent {
    Grabbed,
    Thrown,
    Landed(f32),
    /// Got hit by another stickman.
    Hit { from: u32 },
    /// Something happened in a game you're playing.
    Game(GameEvent),
    /// Another stickman poked us while we were asleep.
    Poked { from: u32 },
    /// Someone high-fived us...
    HighFived { from: u32 },
    /// ...and we high-fived someone.
    GaveHighFive { to: u32 },
    /// Saw `attacker` hit `victim` (it may take sides).
    Saw { attacker: u32, victim: u32 },
    /// You praised (+) or scolded (-) what it's doing.
    Teach(f32),
    /// You stroked it with the mouse.
    Petted,
    /// You're paying attention to it (hovering nearby...).
    Attention(f32),
}

pub use crate::rig::Feeling;

pub trait Brain {
    fn tick(&mut self, dt: f32);
    fn decide(&mut self, p: &Perception, world: &World, rng: &mut Rng) -> Intent;
    fn notify(&mut self, e: BrainEvent);
    /// A random impulse is about to take over (interrupt what it's doing).
    fn whim_due(&self) -> bool;
    /// Something it finished doing and how it went (for the training log).
    fn take_experience(&mut self) -> Option<Experience>;
    /// A new feeling to show (with body language).
    fn take_feeling(&mut self) -> Option<Feeling>;
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
    Kick,
    Sleep,
    PushUps,
    Box,
    Meditate,
    EdgeSit,
    Cartwheel,
    Moonwalk,
    Sprint,
    Trip,
    Tantrum,
    PlayDead,
    Wrap,
    Spin,
    Panic,
    Slide,
    Leap,
    Wake,
    Poke,
    Swing,
    HighFive,
    HangOut,
    Avoid,
}

const ALL: [Kind; 39] = [
    Kind::Idle,
    Kind::Walk,
    Kind::Run,
    Kind::Sit,
    Kind::Wave,
    Kind::Dance,
    Kind::Jump,
    Kind::Enter,
    Kind::Flip,
    Kind::Climb,
    Kind::Drop,
    Kind::Layer,
    Kind::Follow,
    Kind::Cursor,
    Kind::Approach,
    Kind::Punch,
    Kind::Kick,
    Kind::Sleep,
    Kind::PushUps,
    Kind::Box,
    Kind::Meditate,
    Kind::EdgeSit,
    Kind::Cartwheel,
    Kind::Moonwalk,
    Kind::Sprint,
    Kind::Trip,
    Kind::Tantrum,
    Kind::PlayDead,
    Kind::Wrap,
    Kind::Spin,
    Kind::Panic,
    Kind::Slide,
    Kind::Leap,
    Kind::Wake,
    Kind::Poke,
    Kind::Swing,
    Kind::HighFive,
    Kind::HangOut,
    Kind::Avoid,
];

/// What each action is like, for the learning network: energetic, risky,
/// social, aggressive, restful, playful, explore, show-off, affectionate,
/// avoid, stationary, (bias). `training/train.py` has the same table.
fn tags(kind: Kind) -> [f32; TAGS] {
    let t = |e: f32, r: f32, so: f32, ag: f32, re: f32, pl: f32, ex: f32, sh: f32, af: f32, av: f32, st: f32| {
        [e, r, so, ag, re, pl, ex, sh, af, av, st, 1.0]
    };
    match kind {
        Kind::Idle => t(0.0, 0.0, 0.0, 0.0, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        Kind::Walk => t(0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.4, 0.0, 0.0, 0.0, 0.0),
        Kind::Run => t(0.8, 0.1, 0.0, 0.0, 0.0, 0.2, 0.6, 0.0, 0.0, 0.0, 0.0),
        Kind::Sit => t(0.0, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        Kind::Wave => t(0.1, 0.0, 0.5, 0.0, 0.0, 0.2, 0.0, 0.4, 0.2, 0.0, 0.5),
        Kind::Dance => t(0.6, 0.0, 0.1, 0.0, 0.0, 0.9, 0.0, 0.6, 0.0, 0.0, 0.5),
        Kind::Jump => t(0.6, 0.4, 0.0, 0.0, 0.0, 0.2, 0.8, 0.0, 0.0, 0.0, 0.0),
        Kind::Enter => t(0.3, 0.1, 0.0, 0.0, 0.0, 0.1, 1.0, 0.0, 0.0, 0.0, 0.0),
        Kind::Flip => t(0.7, 0.8, 0.0, 0.0, 0.0, 0.6, 0.0, 0.5, 0.0, 0.0, 0.3),
        Kind::Climb => t(0.7, 0.3, 0.0, 0.0, 0.0, 0.1, 0.8, 0.0, 0.0, 0.0, 0.0),
        Kind::Drop => t(0.2, 0.5, 0.0, 0.0, 0.0, 0.2, 0.5, 0.0, 0.0, 0.0, 0.0),
        Kind::Layer => t(0.1, 0.0, 0.0, 0.0, 0.0, 0.2, 0.3, 0.0, 0.0, 0.0, 0.3),
        Kind::Follow => t(0.3, 0.0, 0.2, 0.0, 0.0, 0.3, 0.1, 0.7, 0.0, 0.0, 0.0),
        Kind::Cursor => t(0.5, 0.4, 0.0, 0.0, 0.0, 0.6, 0.0, 1.0, 0.0, 0.0, 0.0),
        Kind::Approach => t(0.3, 0.0, 0.6, 0.0, 0.0, 0.0, 0.2, 0.0, 0.1, 0.0, 0.0),
        Kind::Punch => t(0.5, 0.3, 0.2, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3),
        Kind::Kick => t(0.6, 0.3, 0.2, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3),
        Kind::Sleep => t(0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        Kind::PushUps => t(0.9, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0, 0.3, 0.0, 0.0, 0.6),
        Kind::Box => t(0.6, 0.0, 0.0, 0.3, 0.0, 0.2, 0.0, 0.3, 0.0, 0.0, 0.5),
        Kind::Meditate => t(0.0, 0.0, 0.0, 0.0, 0.9, 0.0, 0.0, 0.1, 0.0, 0.0, 1.0),
        Kind::EdgeSit => t(0.0, 0.1, 0.0, 0.0, 0.6, 0.1, 0.0, 0.0, 0.0, 0.0, 0.9),
        Kind::Cartwheel => t(0.7, 0.5, 0.0, 0.0, 0.0, 0.7, 0.1, 0.5, 0.0, 0.0, 0.0),
        Kind::Moonwalk => t(0.4, 0.0, 0.0, 0.0, 0.0, 0.9, 0.1, 0.7, 0.0, 0.0, 0.0),
        Kind::Sprint => t(1.0, 0.1, 0.0, 0.0, 0.0, 0.3, 0.4, 0.0, 0.0, 0.0, 0.0),
        Kind::Trip => t(0.2, 0.6, 0.0, 0.0, 0.0, 0.5, 0.0, 0.2, 0.0, 0.0, 0.3),
        Kind::Tantrum => t(0.4, 0.2, 0.0, 0.1, 0.0, 0.5, 0.0, 0.3, 0.0, 0.0, 0.6),
        Kind::PlayDead => t(0.0, 0.0, 0.0, 0.0, 0.3, 0.6, 0.0, 0.3, 0.0, 0.0, 1.0),
        Kind::Wrap => t(0.8, 0.1, 0.0, 0.0, 0.0, 0.6, 0.5, 0.2, 0.0, 0.0, 0.0),
        Kind::Spin => t(0.4, 0.1, 0.0, 0.0, 0.0, 0.8, 0.0, 0.4, 0.0, 0.0, 0.8),
        Kind::Panic => t(0.9, 0.1, 0.0, 0.0, 0.0, 0.7, 0.0, 0.3, 0.0, 0.3, 0.0),
        Kind::Slide => t(0.7, 0.4, 0.0, 0.0, 0.0, 0.7, 0.1, 0.3, 0.0, 0.0, 0.0),
        Kind::Leap => t(0.8, 0.6, 0.0, 0.0, 0.0, 0.5, 0.0, 0.5, 0.0, 0.0, 0.2),
        Kind::Wake => t(0.2, 0.0, 0.6, 0.0, 0.0, 0.2, 0.0, 0.0, 0.3, 0.0, 0.0),
        Kind::Poke => t(0.1, 0.0, 0.7, 0.1, 0.0, 0.3, 0.0, 0.0, 0.2, 0.0, 0.5),
        Kind::Swing => t(0.7, 0.5, 0.0, 0.0, 0.0, 0.8, 0.1, 0.4, 0.0, 0.0, 0.0),
        Kind::HighFive => t(0.2, 0.0, 1.0, 0.0, 0.0, 0.3, 0.0, 0.1, 1.0, 0.0, 0.4),
        Kind::HangOut => t(0.1, 0.0, 0.8, 0.0, 0.3, 0.1, 0.0, 0.0, 0.6, 0.0, 0.0),
        Kind::Avoid => t(0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 1.0, 0.0),
    }
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

/// The choice being carried out, and how it's going so far.
struct Pending {
    kind: Kind,
    ctx: [f32; CTX],
    /// What the network expected.
    predicted: f32,
    /// Boredom and health when it started.
    boredom: f32,
    health: f32,
    /// Things that happened along the way (praise, hits, high-fives...).
    reward: f32,
    attention: f32,
    taught: bool,
}

/// Scores every possible action from instinct and from what it has
/// learned, adds curiosity about things it hasn't tried much and some
/// noise, and picks the best.
pub struct UtilityBrain {
    /// Which stickman this is (other stickmen know it by this).
    pub id: u32,
    pub personality: Personality,
    pub needs: Needs,
    current: Kind,
    history: Vec<Kind>,
    /// Who hit us last, and how annoyed we still are (0..1).
    grudge: Option<(u32, f32)>,
    /// An ongoing fight: (opponent, seconds left).
    fight: Option<(u32, f32)>,
    /// A reaction to do next, whatever else it wanted (cheering a win...).
    reaction: Option<(Kind, Intent)>,
    /// Seconds until the next random impulse.
    whim_in: f32,
    /// What it has learned: the network, plus per-action corrections and
    /// how often it has tried each thing.
    net: Net,
    bias: Vec<f32>,
    tries: Vec<u32>,
    experience: u32,
    /// The situation of the decision being made.
    ctx: [f32; CTX],
    health: f32,
    pending: Option<Pending>,
    done: Option<Experience>,
    /// How much it likes each other stickman, -1 (enemy) .. 1 (friend).
    relations: Vec<(u32, f32)>,
    feeling: Option<Feeling>,
}

impl UtilityBrain {
    pub fn new(id: u32, personality: Personality) -> Self {
        UtilityBrain {
            id,
            personality,
            needs: Needs { boredom: 0.3, energy: 0.9 },
            current: Kind::Idle,
            history: Vec::new(),
            grudge: None,
            fight: None,
            reaction: None,
            whim_in: 12.0,
            net: Net::base(),
            bias: vec![0.0; ALL.len()],
            tries: vec![0; ALL.len()],
            experience: 0,
            ctx: [0.0; CTX],
            health: 1.0,
            pending: None,
            done: None,
            relations: Vec::new(),
            feeling: None,
        }
    }

    /// Everything it is and has learned, as text (saved with the stickman).
    pub fn save(&self) -> String {
        let p = self.personality;
        let mut s = format!(
            "personality {:.3} {:.3} {:.3} {:.3}\nneeds {:.3} {:.3}\nexperience {}\n",
            p.curiosity, p.laziness, p.mischief, p.playfulness, self.needs.boredom, self.needs.energy, self.experience
        );
        s += "relations";
        for (id, a) in &self.relations {
            s += &format!(" {id}:{a:.3}");
        }
        s += "\nkinds";
        for (i, k) in ALL.iter().enumerate() {
            s += &format!(" {:?}:{}:{:.4}", k, self.tries[i], self.bias[i]);
        }
        s += "\n";
        s += &self.net.to_text();
        s
    }

    /// The opposite of `save`. None if it's not a saved brain.
    pub fn load(id: u32, text: &str) -> Option<UtilityBrain> {
        let mut personality = None;
        let mut brain = UtilityBrain::new(id, Personality { curiosity: 0.5, laziness: 0.4, mischief: 0.5, playfulness: 0.6 });
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let Some(key) = parts.next() else { continue };
            let nums = || line.split_whitespace().skip(1).filter_map(|v| v.parse::<f32>().ok()).collect::<Vec<f32>>();
            match key {
                "personality" => {
                    let n = nums();
                    if n.len() == 4 {
                        personality = Some(Personality { curiosity: n[0], laziness: n[1], mischief: n[2], playfulness: n[3] });
                    }
                }
                "needs" => {
                    let n = nums();
                    if n.len() == 2 {
                        brain.needs = Needs { boredom: n[0].clamp(0.0, 1.0), energy: n[1].clamp(0.0, 1.0) };
                    }
                }
                "experience" => brain.experience = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                "relations" => {
                    for pair in parts {
                        if let Some((who, a)) = pair.split_once(':') {
                            if let (Ok(who), Ok(a)) = (who.parse::<u32>(), a.parse::<f32>()) {
                                brain.relations.push((who, a.clamp(-1.0, 1.0)));
                            }
                        }
                    }
                }
                "kinds" => {
                    for entry in parts {
                        let mut f = entry.split(':');
                        let (Some(name), Some(n), Some(b)) = (f.next(), f.next(), f.next()) else { continue };
                        if let Some(i) = ALL.iter().position(|k| format!("{k:?}") == name) {
                            brain.tries[i] = n.parse().unwrap_or(0);
                            brain.bias[i] = b.parse::<f32>().unwrap_or(0.0).clamp(-1.0, 1.0);
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(at) = text.find("stickman-net") {
            if let Some(net) = Net::parse(&text[at..]) {
                brain.net = net;
            }
        }
        brain.personality = personality?;
        Some(brain)
    }

    /// How much it likes another stickman, -1..1.
    pub fn affinity(&self, id: u32) -> f32 {
        self.relations.iter().find(|(w, _)| *w == id).map_or(0.0, |(_, a)| *a)
    }

    fn bond(&mut self, id: u32, delta: f32) {
        if id == self.id {
            return;
        }
        match self.relations.iter_mut().find(|(w, _)| *w == id) {
            Some((_, a)) => *a = (*a + delta).clamp(-1.0, 1.0),
            None => self.relations.push((id, delta.clamp(-1.0, 1.0))),
        }
    }

    /// Forget a stickman that's gone for good.
    pub fn forget(&mut self, id: u32) {
        self.relations.retain(|(w, _)| *w != id);
    }

    /// How much it wants to fight someone: fresh anger plus old enmity.
    fn hostility(&self, id: u32) -> f32 {
        self.grudge_against(id) + (-self.affinity(id)).max(0.0) * 0.8
    }

    /// What the network (plus its per-action correction) expects from this.
    fn value(&self, kind: Kind) -> f32 {
        let x = learn::input(&self.ctx, &tags(kind));
        self.net.predict(&x) + self.bias[kind as usize]
    }

    /// How much to go with what it learned over its instincts: some from
    /// the start (the base brain), more as it gains its own experience.
    fn trust(&self) -> f32 {
        let e = self.experience as f32;
        0.25 + 0.4 * e / (e + 150.0)
    }

    /// Something good or bad happened during the current action.
    fn reward(&mut self, r: f32) {
        if let Some(p) = &mut self.pending {
            p.reward += r;
        }
    }

    fn attend(&mut self, amount: f32) {
        if let Some(p) = &mut self.pending {
            let add = amount.min(0.6 - p.attention).max(0.0);
            p.attention += add;
            p.reward += add * 0.6;
        }
    }

    fn feel(&mut self, t: Feeling) {
        self.feeling = Some(t);
    }

    /// The last action is over: see how it went and learn from it.
    fn finish(&mut self) {
        let Some(p) = self.pending.take() else { return };
        let relief = p.boredom - self.needs.boredom;
        let pain = (p.health - self.health).max(0.0);
        let tired = if self.needs.energy < 0.12 { 0.2 } else { 0.0 };
        let reward = (p.reward + relief * 0.8 - pain * 3.0 - tired).clamp(-1.5, 1.5);
        let k = p.kind as usize;
        let x = learn::input(&p.ctx, &tags(p.kind));
        let before = self.net.predict(&x) + self.bias[k];
        // Your praise and scolding count for more than its own experience.
        let lr = if p.taught { 0.08 } else { 0.025 };
        self.net.learn(&x, reward - self.bias[k], lr);
        self.bias[k] = (self.bias[k] - 0.03 * (before - reward)).clamp(-1.0, 1.0);
        self.tries[k] = self.tries[k].saturating_add(1);
        self.experience = self.experience.saturating_add(1);
        // Way off what it expected: that's a surprise.
        if (reward - p.predicted).abs() > 0.7 && self.feeling.is_none() {
            self.feel(if reward > p.predicted { Feeling::Proud } else { Feeling::Startled });
        }
        self.done = Some(Experience { kind: format!("{:?}", p.kind), ctx: p.ctx, tags: tags(p.kind), reward, taught: p.taught });
    }

    /// Something random and silly, out of nowhere.
    fn whim(&mut self, p: &Perception, surf: &Surface, rng: &mut Rng) -> (Kind, Intent) {
        let s = p.scale;
        let margin = 14.0 * s;
        let wide = surf.x1 - surf.x0 > margin * 3.0;
        let random_x = if wide { rng.range(surf.x0 + margin, surf.x1 - margin) } else { p.hip.x };
        let far = if p.hip.x - surf.x0 > surf.x1 - p.hip.x { surf.x0 + margin } else { surf.x1 - margin };
        let away = if p.cursor.x < p.hip.x { surf.x1 - margin } else { surf.x0 + margin };
        let mut picks: Vec<(Kind, Intent)> = vec![
            (Kind::Trip, Intent::Trip),
            (Kind::Tantrum, Intent::Tantrum),
            (Kind::PlayDead, Intent::PlayDead { secs: rng.range(5.0, 12.0) }),
            (Kind::Spin, Intent::Spin { secs: rng.range(1.0, 2.5) }),
        ];
        if p.health >= 0.5 {
            let walk = |x: f32, style: WalkStyle, speed: f32| Intent::Walk { x, speed: speed * s, style };
            picks.extend([
                (Kind::Moonwalk, walk(random_x, WalkStyle::Moonwalk, WalkStyle::Moonwalk.speed())),
                (Kind::Sprint, walk(far, WalkStyle::Normal, 460.0)),
                (Kind::Panic, walk(away, WalkStyle::Panic, WalkStyle::Panic.speed())),
                (Kind::Slide, Intent::Slide),
                (Kind::Leap, Intent::HighLeap),
                (Kind::Flip, Intent::Flip),
            ]);
            if p.support == Support::Floor {
                picks.push((Kind::Wrap, Intent::Wrap { dir: if rng.chance(0.5) { 1.0 } else { -1.0 } }));
            }
            if p.allow_cursor {
                picks.push((Kind::Cursor, Intent::GrabCursor { secs: rng.range(2.0, 5.0) }));
            }
        }
        // Don't do the same silly thing twice in a row.
        picks.retain(|(k, _)| !self.history.iter().rev().take(3).any(|h| h == k));
        if picks.is_empty() {
            return (Kind::Trip, Intent::Trip);
        }
        let i = ((rng.f32() * picks.len() as f32) as usize).min(picks.len() - 1);
        picks[i]
    }

    fn start_fight(&mut self, id: u32, rng_secs: f32) {
        match &mut self.fight {
            Some((who, t)) if *who == id => *t = t.max(rng_secs),
            _ => self.fight = Some((id, rng_secs)),
        }
    }

    /// Mid-fight: keep at it until the timer runs out, someone's too hurt,
    /// or the opponent's gone. Returns None to fall back to normal choices.
    fn fight_move(&mut self, p: &Perception, surf: &Surface, rng: &mut Rng) -> Option<Intent> {
        let (target, _) = self.fight?;
        let s = p.scale;
        let Some(foe) = p.peers.iter().find(|q| q.id == target).copied() else {
            self.fight = None;
            return None;
        };
        if p.health < 0.3 || (foe.hip.y - p.hip.y).abs() > 40.0 * s {
            self.fight = None;
            return None;
        }
        let dx = foe.hip.x - p.hip.x;
        let dir = if dx >= 0.0 { 1.0 } else { -1.0 };
        let pers = self.personality;
        let walk = |x: f32, speed: f32| Intent::Walk {
            x: x.clamp(surf.x0 + 6.0 * s, surf.x1 - 6.0 * s),
            speed: speed * s,
            style: WalkStyle::Normal,
        };

        let mut options: Vec<(Kind, Intent, f32)> = Vec::new();
        if foe.ragdoll {
            // They're down: taunt, square up, or close in for when they get up.
            options.push((Kind::Box, Intent::Pose { secs: 0.7, gesture: Gesture::Box }, 0.5 + pers.mischief * 0.3));
            options.push((Kind::Idle, Intent::Idle { secs: 0.5, fidget: Gesture::None }, 0.4));
            if dx.abs() > 40.0 * s {
                options.push((Kind::Approach, walk(foe.hip.x - dir * 30.0 * s, 140.0), 0.6));
            }
        } else if dx.abs() > 30.0 * s {
            options.push((Kind::Approach, walk(foe.hip.x - dir * 18.0 * s, 280.0), 1.0));
        } else {
            if dx.abs() < 26.0 * s {
                options.push((Kind::Punch, Intent::Punch { target, dir }, 0.7 + pers.mischief * 0.3));
            }
            options.push((Kind::Kick, Intent::Kick { target: Some(target), dir }, 0.55 + pers.mischief * 0.2));
            // Back off a step, or put the guard up, to make room for a counter.
            options.push((Kind::Walk, walk(p.hip.x - dir * 28.0 * s, 160.0), 0.3 + pers.laziness * 0.2));
            options.push((Kind::Box, Intent::Pose { secs: 0.5, gesture: Gesture::Box }, 0.25));
        }
        let mut best = (Kind::Idle, Intent::Idle { secs: 0.3, fidget: Gesture::None }, f32::MIN);
        for (kind, intent, score) in options {
            // Mild variety pressure so it mixes punches, kicks and footwork.
            let repeats = self.history.iter().rev().take(2).filter(|h| **h == kind).count() as i32;
            let score = score * 0.75f32.powi(repeats) + rng.f32() * 0.35;
            if score > best.2 {
                best = (kind, intent, score);
            }
        }
        self.commit(best.0);
        Some(best.1)
    }

    fn grudge_against(&self, id: u32) -> f32 {
        match self.grudge {
            Some((g, amount)) if g == id => amount,
            _ => 0.0,
        }
    }

    fn commit(&mut self, kind: Kind) {
        // Remember what it expects from this, to learn from how it goes.
        self.pending = Some(Pending {
            kind,
            ctx: self.ctx,
            predicted: self.value(kind),
            boredom: self.needs.boredom,
            health: self.health,
            reward: 0.0,
            attention: 0.0,
            taught: false,
        });
        let n = &mut self.needs;
        match kind {
            Kind::Jump | Kind::Climb | Kind::Flip | Kind::Enter | Kind::Cursor | Kind::Swing => {
                n.energy -= 0.1;
                n.boredom -= 0.35;
            }
            Kind::Run | Kind::Dance | Kind::PushUps | Kind::Box | Kind::Cartwheel => {
                n.energy -= 0.06;
                n.boredom -= 0.25;
            }
            Kind::Sleep | Kind::Meditate | Kind::EdgeSit => n.boredom -= 0.1,
            Kind::Punch | Kind::Kick => {
                n.boredom -= 0.3;
                if let Some((_, g)) = &mut self.grudge {
                    *g -= 0.35;
                }
            }
            Kind::HighFive => n.boredom -= 0.25,
            Kind::HangOut => n.boredom -= 0.12,
            Kind::Drop
            | Kind::Layer
            | Kind::Wave
            | Kind::Follow
            | Kind::Approach
            | Kind::Wake
            | Kind::Poke
            | Kind::Avoid => n.boredom -= 0.2,
            Kind::Moonwalk
            | Kind::Sprint
            | Kind::Trip
            | Kind::Tantrum
            | Kind::PlayDead
            | Kind::Wrap
            | Kind::Spin
            | Kind::Panic
            | Kind::Slide
            | Kind::Leap => {
                n.boredom -= 0.3;
                n.energy -= 0.03;
            }
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
            Kind::Sleep => {
                n.energy += 0.1 * dt;
                n.boredom += 0.01 * dt;
            }
            Kind::Sit | Kind::Meditate | Kind::EdgeSit | Kind::HangOut => {
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
        if self.current != Kind::Sleep {
            self.whim_in -= dt;
        }
        if let Some((_, t)) = &mut self.fight {
            *t -= dt;
        }
        if matches!(self.fight, Some((_, t)) if t <= 0.0) {
            self.fight = None;
        }
    }

    fn notify(&mut self, e: BrainEvent) {
        match e {
            BrainEvent::Grabbed => {
                self.needs.boredom = (self.needs.boredom - 0.3).max(0.0);
                self.current = Kind::Idle;
                self.attend(0.3);
                self.feel(Feeling::Startled);
            }
            BrainEvent::Thrown => self.needs.boredom = (self.needs.boredom - 0.2).max(0.0),
            BrainEvent::Landed(impact) => {
                if impact > 1500.0 {
                    self.needs.energy = (self.needs.energy - 0.05).max(0.0);
                }
            }
            BrainEvent::Hit { from } => {
                let g = (self.grudge_against(from) + 0.5 + self.personality.mischief * 0.3).min(1.0);
                self.grudge = Some((from, g));
                self.current = Kind::Idle;
                self.bond(from, -0.15);
                self.reward(-0.4);
                self.feel(Feeling::Angry);
                // Hit back: fight for a while (longer if it's a grudge match).
                self.start_fight(from, 6.0 + g * 6.0);
            }
            BrainEvent::Game(GameEvent::Finished(_, outcome)) => {
                let pose = |secs: f32, gesture: Gesture| Intent::Pose { secs, gesture };
                self.reaction = Some(match outcome {
                    // You won: celebrate.
                    Outcome::Won => (Kind::Dance, pose(4.0, Gesture::Dance)),
                    // You lost: sulk.
                    Outcome::Lost => (Kind::Sit, Intent::Sit { secs: 4.0 }),
                    Outcome::Over => (Kind::Wave, pose(1.5, Gesture::Wave)),
                });
                self.feel(if outcome == Outcome::Lost { Feeling::Sad } else { Feeling::Happy });
                self.needs.boredom = (self.needs.boredom - 0.3).max(0.0);
            }
            BrainEvent::Game(GameEvent::Opened(..)) => {
                // Ooh, a game: curiosity spikes (it'll want to go watch).
                self.needs.boredom = (self.needs.boredom + 0.3).min(1.0);
                self.feel(Feeling::Startled);
            }
            BrainEvent::Poked { from } => {
                // Woken up. Usually a stretch; sometimes a fright; sometimes grumpy.
                let roll = (self.whim_in * 7.31).sin().abs();
                let friendly = self.affinity(from) > 0.3;
                self.reaction = Some(if roll < 0.55 || friendly {
                    (Kind::Idle, Intent::Idle { secs: 2.0, fidget: Gesture::Stretch })
                } else if roll < 0.8 {
                    (Kind::Flip, Intent::Flip)
                } else {
                    self.start_fight(from, 4.0);
                    (Kind::Box, Intent::Pose { secs: 0.8, gesture: Gesture::Box })
                });
                if !friendly {
                    self.bond(from, -0.02);
                }
                self.feel(Feeling::Startled);
                self.needs.energy = (self.needs.energy + 0.2).min(1.0);
            }
            BrainEvent::HighFived { from } => {
                if self.affinity(from) > -0.3 {
                    // Up top!
                    self.bond(from, 0.1);
                    self.reward(0.3);
                    self.reaction = Some((Kind::HighFive, Intent::Pose { secs: 0.7, gesture: Gesture::HighFive(1.0) }));
                    self.feel(Feeling::Happy);
                } else {
                    // Leaves them hanging.
                    self.bond(from, -0.02);
                    self.feel(Feeling::Angry);
                }
            }
            BrainEvent::GaveHighFive { to } => {
                self.bond(to, 0.06);
                self.reward(0.3);
                self.feel(Feeling::Happy);
            }
            BrainEvent::Saw { attacker, victim } => {
                if attacker == self.id || victim == self.id {
                    return;
                }
                let fv = self.affinity(victim);
                let fa = self.affinity(attacker);
                if fv > 0.35 {
                    // Someone hit my friend.
                    self.bond(attacker, -0.1 * fv);
                    if fa < 0.3 && self.personality.mischief + fv > 0.8 {
                        self.start_fight(attacker, 5.0);
                        self.feel(Feeling::Angry);
                    }
                } else if fv < -0.3 && fa > 0.0 {
                    // The enemy of my enemy...
                    self.bond(attacker, 0.03);
                }
            }
            BrainEvent::Teach(v) => {
                if let Some(p) = &mut self.pending {
                    p.reward += v;
                    p.taught = true;
                }
                if v > 0.0 {
                    self.feel(Feeling::Happy);
                } else {
                    self.feel(Feeling::Sad);
                    // Sulks for a moment.
                    self.reaction = Some((Kind::Sit, Intent::Sit { secs: 2.5 }));
                }
            }
            BrainEvent::Petted => {
                if let Some(p) = &mut self.pending {
                    p.reward += 0.4;
                    p.taught = true;
                }
                self.needs.boredom = (self.needs.boredom - 0.1).max(0.0);
                self.feel(Feeling::Happy);
            }
            BrainEvent::Attention(a) => self.attend(a),
        }
    }

    fn take_experience(&mut self) -> Option<Experience> {
        self.done.take()
    }

    fn take_feeling(&mut self) -> Option<Feeling> {
        self.feeling.take()
    }

    fn whim_due(&self) -> bool {
        // Never in the middle of a fight or a reaction (it used to interrupt
        // every punch mid-swing).
        self.whim_in <= 0.0 && self.fight.is_none() && self.reaction.is_none()
    }

    fn decide(&mut self, p: &Perception, world: &World, rng: &mut Rng) -> Intent {
        let s = p.scale;
        // How did the last thing go? Learn from it.
        self.health = p.health;
        self.finish();
        let Some(surf) = world.surface_near(p.support, p.foot.y) else {
            return Intent::Idle { secs: 0.5, fidget: Gesture::None };
        };
        // Who's around. Spending time near someone slowly makes friends
        // (less so for troublemakers).
        let near: Vec<(u32, bool)> =
            p.peers.iter().filter(|q| (q.hip - p.hip).len() < 400.0 * s).map(|q| (q.id, q.sleeping)).collect();
        let familiarity = 0.004 * (1.0 - self.personality.mischief * 0.6);
        for (id, _) in &near {
            self.bond(*id, familiarity);
        }
        let friend = near.iter().map(|(id, _)| self.affinity(*id)).fold(0.0f32, f32::max);
        let enemy = near.iter().map(|(id, _)| -self.affinity(*id)).fold(0.0f32, f32::max);
        // The situation, as the learning network sees it (see learn.rs).
        {
            let pers = self.personality;
            let flag = |b: bool| if b { 1.0 } else { 0.0 };
            self.ctx = [
                self.needs.boredom,
                self.needs.energy,
                p.health,
                pers.curiosity,
                pers.laziness,
                pers.mischief,
                pers.playfulness,
                flag(matches!(p.support, Support::Inside(_) | Support::Ledge { .. })),
                flag((p.cursor - p.hip).len() < 250.0 * s),
                friend.min(1.0),
                enemy.min(1.0),
                flag(near.iter().any(|(_, sleeping)| *sleeping)),
                p.shaky.clamp(0.0, 1.0),
                p.height.clamp(0.0, 1.0),
                p.attention.clamp(0.0, 1.0),
                1.0,
            ];
        }
        if let Some((kind, intent)) = self.reaction.take() {
            self.commit(kind);
            return intent;
        }
        if let Some(intent) = self.fight_move(p, &surf, rng) {
            return intent;
        }
        // Moods drift: the same stickman is a bit different from minute to minute.
        for t in [
            &mut self.personality.curiosity,
            &mut self.personality.laziness,
            &mut self.personality.mischief,
            &mut self.personality.playfulness,
        ] {
            *t = (*t + rng.range(-0.03, 0.03)).clamp(0.05, 0.95);
        }
        // Out of nowhere: a random impulse.
        if self.whim_in <= 0.0 {
            let pers = self.personality;
            self.whim_in = rng.range(5.0, 22.0) * (1.3 - pers.mischief * 0.6 - pers.playfulness * 0.3).max(0.3);
            let (kind, intent) = self.whim(p, &surf, rng);
            self.commit(kind);
            return intent;
        }
        // Watching you play a game: go there, and hang around once there.
        let watching = p.focus_game;
        let at_game = watching.is_some() && p.support.window() == watching;
        let pers = self.personality;
        let (b, e) = (self.needs.boredom, self.needs.energy);
        // Hurt stickmen are less keen on anything energetic.
        let ef = e.max(0.2) * p.health.clamp(0.25, 1.0);
        let hurt = 1.0 - p.health;
        // Pick a walking style from mood: tired shuffles, playful struts or
        // skips, mischievous sneaks.
        let style = {
            let r = rng.f32();
            if e < 0.3 {
                WalkStyle::Shuffle
            } else if r < 0.15 + pers.playfulness * 0.15 {
                WalkStyle::Strut
            } else if r < 0.3 + pers.mischief * 0.15 {
                WalkStyle::Sneak
            } else if r < 0.4 + pers.mischief * 0.15 {
                WalkStyle::March
            } else if r < 0.5 + pers.playfulness * 0.1 + b * 0.1 {
                WalkStyle::Skip
            } else {
                WalkStyle::Normal
            }
        };
        let walk = |x: f32| Intent::Walk { x, speed: style.speed() * s, style };
        let mut options: Vec<(Kind, Intent, f32)> = Vec::new();
        // Inside a window: on its floor, or on boxes of the web page in it.
        let inside = matches!(p.support, Support::Inside(_) | Support::Ledge { .. });
        let room = if inside { p.support.host() } else { None };

        // Idle, with a random fidget (or just looking around).
        let fidget = match (rng.f32() * 7.0) as u32 {
            0 => Gesture::Stretch,
            1 => Gesture::Scratch,
            2 if b > 0.4 => Gesture::TapFoot,
            _ => Gesture::None,
        };
        let idle_secs = if fidget == Gesture::None { rng.range(1.0, 3.5) } else { rng.range(1.5, 3.0) };
        options.push((
            Kind::Idle,
            Intent::Idle { secs: idle_secs, fidget },
            0.35 + pers.laziness * 0.3 - b * 0.35,
        ));
        options.push((
            Kind::Sit,
            Intent::Sit { secs: rng.range(3.0, 9.0) },
            (1.0 - e) * 0.9 + pers.laziness * 0.2 - b * 0.25 + hurt * 0.3,
        ));
        let pose = |secs: f32, gesture: Gesture| Intent::Pose { secs, gesture };
        options.push((
            Kind::Sleep,
            pose(rng.range(6.0, 14.0), Gesture::Sleep),
            (1.0 - e) * 1.1 + pers.laziness * 0.3 - b * 0.3 - 0.35 + hurt * 0.7,
        ));
        options.push((
            Kind::Meditate,
            pose(rng.range(4.0, 8.0), Gesture::Meditate),
            0.06 + (1.0 - pers.mischief) * 0.2 + (1.0 - e) * 0.2,
        ));
        options.push((
            Kind::Dance,
            pose(rng.range(2.0, 4.5), Gesture::Dance),
            (0.08 + pers.playfulness * 0.3 + b * 0.15) * ef,
        ));
        options.push((
            Kind::PushUps,
            pose(rng.range(3.0, 5.0), Gesture::PushUp),
            (0.08 + pers.playfulness * 0.2 + b * 0.2) * e,
        ));
        options.push((
            Kind::Box,
            pose(rng.range(1.5, 3.0), Gesture::Box),
            (0.08 + pers.mischief * 0.25 + b * 0.15) * ef,
        ));
        let kick_dir = if rng.chance(0.5) { 1.0 } else { -1.0 };
        options.push((Kind::Kick, Intent::Kick { target: None, dir: kick_dir }, (0.06 + pers.mischief * 0.2) * ef));
        options.push((Kind::Flip, Intent::Flip, (0.1 + pers.playfulness * 0.35) * ef));
        // Cartwheel only if there's room ahead on this surface (either way).
        let wheel_dir = if rng.chance(0.5) { 1.0 } else { -1.0 };
        let land_x = p.hip.x + wheel_dir * 90.0 * s;
        if land_x > surf.x0 + 10.0 * s && land_x < surf.x1 - 10.0 * s {
            options.push((Kind::Cartwheel, Intent::Cartwheel, (0.08 + pers.playfulness * 0.3) * ef));
        }
        // Sit on the edge of the window (or page box) we're standing on, legs
        // over the side.
        if let Support::Window(_) | Support::Ledge { .. } | Support::Icon(_) = p.support {
            let (edge, dir) = if p.hip.x - surf.x0 < surf.x1 - p.hip.x { (surf.x0, -1.0) } else { (surf.x1, 1.0) };
            options.push((
                Kind::EdgeSit,
                Intent::EdgeSit { secs: rng.range(4.0, 10.0), edge, dir },
                0.15 + pers.laziness * 0.3 + pers.curiosity * 0.1,
            ));
        }

        let margin = 14.0 * s;
        if surf.x1 - surf.x0 > margin * 3.0 {
            let x = rng.range(surf.x0 + margin, surf.x1 - margin);
            options.push((Kind::Walk, walk(x), 0.45 + pers.curiosity * 0.15 - hurt * 0.2));
            let far = if p.hip.x - surf.x0 > surf.x1 - p.hip.x { surf.x0 + margin } else { surf.x1 - margin };
            options.push((
                Kind::Run,
                Intent::Walk { x: far, speed: 330.0 * s, style: WalkStyle::Normal },
                (b * 0.6 + pers.playfulness * 0.3) * ef,
            ));
        }

        // Best jump to another surface. Only arcs that don't clip through a
        // window count: otherwise it has to go around (climb, drop, taskbar).
        // Window interiors are reached through the taskbar, not by jumping.
        // Inside a window it only jumps around inside it (between the boxes
        // of a web page, and down to the window's floor).
        let foreground = world.windows.first().map(|w| w.id);
        {
            let mut best_jump: Option<(Intent, f32)> = None;
            let skip = world.overlapping(p.foot, s);
            for t in world.surfaces() {
                let on_head = matches!(t.support, Support::Peer(_));
                let in_window = matches!(t.support, Support::Inside(_) | Support::Ledge { .. });
                let fits = if inside { in_window && t.support.host() == room } else { !in_window };
                // Page boxes and icons can be narrow (tiles, cards); windows can't.
                let min_w = if matches!(t.support, Support::Ledge { .. } | Support::Icon(_)) { 20.0 * s } else { 40.0 * s };
                if t.support == p.support || !fits || (t.x1 - t.x0 < min_w && !on_head) {
                    continue;
                }
                // Only page boxes you can actually see (not scrolled away).
                if let Support::Ledge { win, .. } = t.support {
                    if !world.page_visible(win, t.y, s) {
                        continue;
                    }
                }
                // Another stickman's head: aim for the middle (and stack up).
                // Friends are fun to stand on; enemies, a way to annoy them.
                let x = if on_head { (t.x0 + t.x1) * 0.5 } else { rng.range(t.x0 + 15.0 * s, t.x1 - 15.0 * s) };
                let rise = p.foot.y - t.y;
                let dx = (x - p.hip.x).abs();
                if rise > p.max_jump || dx > 650.0 * s || (rise.abs() < 10.0 * s && dx < 30.0 * s) {
                    continue;
                }
                let target = v2(x, t.y);
                let (vel, flight) = crate::stickman::ballistic(p.foot, target, 35.0 * s, p.gravity);
                if !world.arc_clear(p.layer, p.foot, vel, p.gravity, flight, s, &skip) {
                    continue;
                }
                let head_bonus = match t.support {
                    Support::Peer(id) => {
                        pers.playfulness * 0.3 + pers.mischief * 0.15 + self.affinity(id).abs() * 0.2
                    }
                    _ => 0.0,
                };
                let to_game = (if watching.is_some() && t.support.window() == watching { 0.5 } else { 0.0 })
                    + head_bonus
                    // Exploring a web page: hopping from box to box.
                    + (if matches!(t.support, Support::Ledge { .. }) { 0.15 + pers.curiosity * 0.35 } else { 0.0 })
                    // Playing on the desktop icons.
                    + (if matches!(t.support, Support::Icon(_)) { 0.1 + pers.playfulness * 0.3 } else { 0.0 });
                let score = (0.3 + b * 0.5 + pers.playfulness * 0.3 + (rise.max(0.0) / p.max_jump) * pers.curiosity * 0.2) * ef
                    - dx / (3000.0 * s)
                    + to_game
                    + rng.f32() * 0.1;
                if best_jump.map_or(true, |(_, bs)| score > bs) {
                    best_jump = Some((Intent::Jump { target, support: t.support }, score));
                }
            }
            if let Some((i, sc)) = best_jump {
                options.push((Kind::Jump, i, sc));
            }
        }

        // Taskbar travel: dive into the taskbar, pop out on/in another window.
        if p.support == Support::Floor && world.portal_edge().is_some() {
            let mut best_portal: Option<(Intent, f32)> = None;
            // Only windows that have their own button on the taskbar.
            for w in world.windows.iter().filter(|w| world.door(w.id).is_some()) {
                for target in [Support::Inside(w.id), Support::Window(w.id)] {
                    if world.surface(target).is_none() {
                        continue;
                    }
                    let fg = if foreground == Some(w.id) { 0.15 } else { 0.0 };
                    let room = if matches!(target, Support::Inside(_)) { pers.curiosity * 0.15 } else { 0.0 };
                    let to_game = if watching == Some(w.id) { 0.6 } else { 0.0 };
                    // A web page to explore.
                    let web = if matches!(target, Support::Inside(_)) && world.pages.iter().any(|pg| pg.win == w.id) {
                        0.2 + pers.curiosity * 0.3
                    } else {
                        0.0
                    };
                    let score = (0.15 + pers.curiosity * 0.35 + b * 0.35 + fg + room) * ef + to_game + web + rng.f32() * 0.15;
                    if best_portal.map_or(true, |(_, bs)| score > bs) {
                        best_portal = Some((Intent::Portal { target }, score));
                    }
                }
            }
            if let Some((i, sc)) = best_portal {
                options.push((Kind::Enter, i, sc));
            }
        }

        // Inside a window: climb its inner wall and out over the top.
        if let Support::Inside(id) = p.support {
            if world.surface(Support::Window(id)).is_some() {
                for side in [Side::Left, Side::Right] {
                    if world.wall_x(id, side).is_some() {
                        options.push((
                            Kind::Climb,
                            Intent::Climb { win: id, side, inner: true },
                            (0.15 + pers.curiosity * 0.3 + b * 0.4) * ef + rng.f32() * 0.1,
                        ));
                    }
                }
            }
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
                        best_climb = Some((Intent::Climb { win: w.id, side, inner: false }, score));
                    }
                }
            }
            if let Some((i, sc)) = best_climb {
                options.push((Kind::Climb, i, sc));
            }
        }

        // Kick a desktop icon at body height across the screen (only if
        // you've allowed it to move your icons).
        if p.move_icons && !inside {
            let mut best_kick: Option<(Kind, Intent, f32)> = None;
            for (_, r) in &world.icons {
                if !(r.t < p.foot.y && r.b > p.hip.y - 10.0 * s) {
                    continue;
                }
                let dx = (r.l + r.r) * 0.5 - p.hip.x;
                let dir = if dx >= 0.0 { 1.0 } else { -1.0 };
                let option = if dx.abs() < 34.0 * s {
                    (Kind::Kick, Intent::Kick { target: None, dir }, 0.25 + pers.mischief * 0.5)
                } else if dx.abs() < 400.0 * s && r.l > surf.x0 && r.r < surf.x1 {
                    let x = (r.l + r.r) * 0.5 - dir * 24.0 * s;
                    (Kind::Approach, Intent::Walk { x, speed: 140.0 * s, style: WalkStyle::Sneak }, pers.mischief * 0.35)
                } else {
                    continue;
                };
                if best_kick.as_ref().map_or(true, |b| option.2 > b.2) {
                    best_kick = Some(option);
                }
            }
            options.extend(best_kick);
        }

        // Swing on a desktop icon like a gymnastics bar: jump up, grab its
        // bottom edge, swing higher and higher, and launch off.
        if !inside {
            let mut best_swing: Option<(Intent, f32)> = None;
            for (_, r) in &world.icons {
                let at = v2((r.l + r.r) * 0.5, r.b);
                let above = p.foot.y - at.y;
                let dx = (at.x - p.hip.x).abs();
                if above < p.reach || above > p.max_jump + p.reach || dx > 300.0 * s {
                    continue;
                }
                let score = (0.15 + pers.playfulness * 0.45 + b * 0.2) * ef - dx / (2000.0 * s) + rng.f32() * 0.1;
                if best_swing.map_or(true, |(_, bs)| score > bs) {
                    best_swing = Some((Intent::Swing { at, secs: rng.range(2.5, 5.0) }, score));
                }
            }
            if let Some((i, sc)) = best_swing {
                options.push((Kind::Swing, i, sc));
            }
        }

        // Step off the edge of the window (or page box, or icon) we're on.
        if let Support::Window(_) | Support::Ledge { .. } | Support::Icon(_) = p.support {
            let x = if rng.chance(0.5) { surf.x0 - 12.0 * s } else { surf.x1 + 12.0 * s };
            options.push((Kind::Drop, walk(x), 0.12 + pers.mischief * 0.35 + b * 0.2 - hurt * 0.3));
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
                Intent::Walk { x: p.cursor.x, speed: 150.0 * s, style: WalkStyle::Normal },
                pers.curiosity * 0.35 + b * 0.1,
            ));
        }
        let above = p.foot.y - p.cursor.y;
        if p.allow_cursor
            && !inside
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

        // Someone's asleep nearby: go and wake them (gently, if they're a
        // friend; with a kick, if they're not).
        for peer in p.peers.iter().filter(|q| q.sleeping && (q.ground - p.foot.y).abs() < 12.0 * s) {
            let dx = peer.hip.x - p.hip.x;
            let dir = if dx >= 0.0 { 1.0 } else { -1.0 };
            let aff = self.affinity(peer.id);
            if dx.abs() < 26.0 * s {
                options.push((
                    Kind::Poke,
                    Intent::Poke { target: peer.id, dir },
                    0.35 + pers.playfulness * 0.3 + pers.mischief * 0.2 + aff.max(0.0) * 0.3,
                ));
                options.push((
                    Kind::Kick,
                    Intent::Kick { target: Some(peer.id), dir },
                    pers.mischief * 0.45 - 0.1 + (-aff).max(0.0) * 0.5 - aff.max(0.0) * 0.6,
                ));
            } else if dx.abs() < 700.0 * s && peer.hip.x > surf.x0 && peer.hip.x < surf.x1 {
                options.push((
                    Kind::Wake,
                    Intent::Walk { x: peer.hip.x - dir * 18.0 * s, speed: 110.0 * s, style: WalkStyle::Sneak },
                    0.12 + pers.playfulness * 0.25 + pers.mischief * 0.2,
                ));
            }
        }

        // Other stickmen on the same level: friends get high-fives and
        // company, enemies get fought (or avoided).
        for peer in &p.peers {
            if peer.ragdoll || (peer.hip.y - p.hip.y).abs() > 8.0 * s {
                continue;
            }
            let dx = peer.hip.x - p.hip.x;
            let dir = if dx >= 0.0 { 1.0 } else { -1.0 };
            let aff = self.affinity(peer.id);
            let friendly = aff.max(0.0);
            let host = self.hostility(peer.id);
            if dx.abs() < 400.0 * s {
                wave = wave.max(0.15 + pers.playfulness * 0.2 + friendly * 0.3);
            }
            if dx.abs() < 26.0 * s {
                options.push((
                    Kind::Punch,
                    Intent::Punch { target: peer.id, dir },
                    0.1 + pers.mischief * 0.5 + host * 0.8 - friendly * 0.9,
                ));
            }
            if dx.abs() < 34.0 * s {
                options.push((
                    Kind::Kick,
                    Intent::Kick { target: Some(peer.id), dir },
                    0.08 + pers.mischief * 0.4 + host * 0.7 - friendly * 0.9,
                ));
                if aff > 0.15 {
                    options.push((
                        Kind::HighFive,
                        Intent::HighFive { target: peer.id, dir },
                        0.2 + pers.playfulness * 0.3 + aff * 0.6,
                    ));
                }
            } else if dx.abs() < 600.0 * s && peer.hip.x > surf.x0 && peer.hip.x < surf.x1 {
                let speed = if host > 0.2 { 300.0 } else { 120.0 } * s;
                options.push((
                    Kind::Approach,
                    Intent::Walk { x: peer.hip.x - dir * 18.0 * s, speed, style: WalkStyle::Normal },
                    0.15 + pers.curiosity * 0.25 + host * 0.6 + friendly * 0.2,
                ));
                if aff > 0.2 {
                    options.push((
                        Kind::HangOut,
                        Intent::Walk { x: peer.hip.x - dir * 22.0 * s, speed: 110.0 * s, style: WalkStyle::Normal },
                        0.12 + aff * 0.5 + (1.0 - e) * 0.15,
                    ));
                }
            }
            if aff < -0.3 && dx.abs() < 320.0 * s {
                let away = if dx >= 0.0 { surf.x0 + margin } else { surf.x1 - margin };
                options.push((
                    Kind::Avoid,
                    Intent::Walk { x: away, speed: 200.0 * s, style: WalkStyle::Normal },
                    0.1 + (-aff) * 0.4 + hurt * 0.6 + pers.laziness * 0.2,
                ));
            }
        }
        options.push((Kind::Wave, pose(rng.range(1.2, 2.2), Gesture::Wave), wave));

        if p.support == Support::Floor && !world.windows.is_empty() {
            options.push((Kind::Layer, Intent::ToggleLayer, 0.08 + pers.mischief * 0.25));
        }

        // Every so often, ignore the scores entirely and just do something.
        if !options.is_empty() && rng.chance(0.08) {
            let i = ((rng.f32() * options.len() as f32) as usize).min(options.len() - 1);
            let (kind, intent, _) = options[i];
            self.commit(kind);
            return intent;
        }
        let trust = self.trust();
        let mut best = (Kind::Idle, Intent::Idle { secs: 1.0, fidget: Gesture::None }, f32::MIN);
        for (kind, intent, score) in options {
            let repeats = self.history.iter().filter(|h| **h == kind).count() as i32;
            // At the game you're playing: settle in and watch.
            let watch = if at_game {
                match kind {
                    Kind::Idle | Kind::Sit | Kind::EdgeSit => 0.35,
                    Kind::Drop | Kind::Jump | Kind::Enter | Kind::Run | Kind::Layer | Kind::Climb => -0.35,
                    _ => 0.0,
                }
            } else {
                0.0
            };
            // Instinct, blended with what it has learned to expect from
            // this here, plus curiosity about things it's rarely tried.
            let instinct = score * 0.6f32.powi(repeats) + watch;
            let learned = self.value(kind);
            let curious = 0.18 * pers.curiosity / (1.0 + self.tries[kind as usize] as f32).sqrt();
            let score = instinct * (1.0 - trust) + learned * trust + curious + rng.f32() * 0.3;
            if score > best.2 {
                best = (kind, intent, score);
            }
        }
        // Trying something it hardly knows: "hmm?"
        if self.tries[best.0 as usize] < 2 && !matches!(best.0, Kind::Idle | Kind::Walk) && self.feeling.is_none() {
            self.feel(Feeling::Puzzled);
        }
        self.commit(best.0);
        // Throwing the first punch/kick at someone starts a proper fight
        // (not with friends: that's just messing about).
        let opened = match best.1 {
            Intent::Punch { target, .. } | Intent::Kick { target: Some(target), .. } => Some(target),
            _ => None,
        };
        if let Some(target) = opened {
            if self.affinity(target) < 0.3 {
                let secs = rng.range(5.0, 9.0) + self.hostility(target) * 4.0;
                self.start_fight(target, secs);
                self.bond(target, -0.04);
            }
        }
        best.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brain_saves_and_loads() {
        let mut b = UtilityBrain::new(7, Personality { curiosity: 0.3, laziness: 0.2, mischief: 0.8, playfulness: 0.6 });
        b.bond(3, 0.5);
        b.bond(9, -0.4);
        b.tries[Kind::Flip as usize] = 12;
        b.bias[Kind::Flip as usize] = -0.25;
        let text = b.save();
        let back = UtilityBrain::load(7, &text).expect("load");
        assert!((back.affinity(3) - 0.5).abs() < 1e-3);
        assert!((back.affinity(9) + 0.4).abs() < 1e-3);
        assert!((back.personality.mischief - 0.8).abs() < 1e-3);
        assert_eq!(back.tries[Kind::Flip as usize], 12);
        assert!((back.bias[Kind::Flip as usize] + 0.25).abs() < 1e-3);
        assert!(UtilityBrain::load(1, "nonsense").is_none());
    }

    #[test]
    fn every_kind_has_tags() {
        assert_eq!(ALL.len(), Kind::Avoid as usize + 1);
        for (i, k) in ALL.iter().enumerate() {
            assert_eq!(*k as usize, i);
            assert_eq!(tags(*k)[TAGS - 1], 1.0);
        }
    }
}
