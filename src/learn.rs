//! The stickman's learning brain: a tiny neural network that predicts how
//! much it will enjoy doing something in the situation it's in.
//!
//! * Input: 16 numbers describing the situation (its mood and needs, its
//!   health, whether a friend or an enemy is nearby, whether the thing it's
//!   standing on is moving...) plus 12 describing the action (energetic,
//!   risky, friendly, aggressive, restful...).
//! * One hidden layer of 20 neurons, one output: "how good will this be".
//!   About 600 numbers in total, so a decision costs a few microseconds.
//!
//! It starts from a *base* brain trained by `training/train.py` up to a
//! deliberate cutoff (just the basics: rest when tired, don't do flips when
//! hurt, be nice to friends). From then on every stickman keeps learning on
//! its own: after each thing it does it compares what it expected with what
//! actually happened (did it get hurt, have fun, make a friend, did you pay
//! attention, did you praise or scold it) and nudges its weights. Each
//! stickman ends up with its own brain, saved with it.

use std::io::Write;
use std::path::PathBuf;

/// Numbers describing the situation.
pub const CTX: usize = 16;
/// Numbers describing an action.
pub const TAGS: usize = 12;
pub const INPUTS: usize = CTX + TAGS;
pub const HIDDEN: usize = 20;

pub const CTX_NAMES: [&str; CTX] = [
    "boredom",
    "energy",
    "health",
    "curiosity",
    "laziness",
    "mischief",
    "playfulness",
    "inside_window",
    "cursor_near",
    "friend_near",
    "enemy_near",
    "sleeper_near",
    "shaky_ground",
    "height",
    "attention",
    "bias",
];

pub const TAG_NAMES: [&str; TAGS] = [
    "energetic",
    "risky",
    "social",
    "aggressive",
    "restful",
    "playful",
    "explore",
    "show_off",
    "affectionate",
    "avoid",
    "stationary",
    "bias",
];

/// The base brain, trained by `training/train.py` (GitHub Actions runs it
/// before every build). If it hasn't been trained, each stickman starts
/// from a blank brain instead and learns everything itself.
const BASE: &str = include_str!("base_brain.txt");

fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("stickman"))
}

/// One hidden layer: inputs -> tanh -> one value.
#[derive(Clone, Debug)]
pub struct Net {
    w1: Vec<f32>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: f32,
}

pub fn input(ctx: &[f32; CTX], tags: &[f32; TAGS]) -> [f32; INPUTS] {
    let mut x = [0.0; INPUTS];
    x[..CTX].copy_from_slice(ctx);
    x[CTX..].copy_from_slice(tags);
    x
}

impl Net {
    /// What a new stickman starts with: your own fine-tuned base
    /// (`%APPDATA%\stickman\base_brain.txt`, from `train.py finetune`), else
    /// the one built into the program, else a blank one.
    pub fn base() -> Net {
        dir()
            .and_then(|d| std::fs::read_to_string(d.join("base_brain.txt")).ok())
            .and_then(|t| Net::parse(&t))
            .or_else(|| Net::parse(BASE))
            .unwrap_or_else(Net::blank)
    }

    /// Whether the built-in base brain was trained (false = learning from scratch).
    pub fn has_trained_base() -> bool {
        Net::parse(BASE).is_some()
    }

    /// Tiny random weights: knows nothing, predicts ~0 for everything.
    pub fn blank() -> Net {
        // Fixed little generator so it doesn't need an Rng passed in.
        let mut seed = 0x2545_F491u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 - 0.5) * 0.3
        };
        Net {
            w1: (0..HIDDEN * INPUTS).map(|_| next()).collect(),
            b1: vec![0.0; HIDDEN],
            w2: (0..HIDDEN).map(|_| next() * 0.3).collect(),
            b2: 0.0,
        }
    }

    fn hidden(&self, x: &[f32; INPUTS]) -> [f32; HIDDEN] {
        let mut h = [0.0f32; HIDDEN];
        for (j, hj) in h.iter_mut().enumerate() {
            let row = &self.w1[j * INPUTS..(j + 1) * INPUTS];
            let z: f32 = self.b1[j] + row.iter().zip(x.iter()).map(|(w, x)| w * x).sum::<f32>();
            *hj = z.tanh();
        }
        h
    }

    /// How good it expects this to be (roughly -1..1).
    pub fn predict(&self, x: &[f32; INPUTS]) -> f32 {
        let h = self.hidden(x);
        self.b2 + h.iter().zip(self.w2.iter()).map(|(h, w)| h * w).sum::<f32>()
    }

    /// Learn from one experience: move the prediction for `x` toward
    /// `target`. Returns the prediction before learning.
    pub fn learn(&mut self, x: &[f32; INPUTS], target: f32, lr: f32) -> f32 {
        let h = self.hidden(x);
        let y = self.b2 + h.iter().zip(self.w2.iter()).map(|(h, w)| h * w).sum::<f32>();
        // Clipped so one freak experience can't wreck everything.
        let err = (y - target).clamp(-1.0, 1.0);
        for j in 0..HIDDEN {
            let dh = err * self.w2[j] * (1.0 - h[j] * h[j]);
            self.w2[j] -= lr * (err * h[j] + 1e-4 * self.w2[j]);
            self.b1[j] -= lr * dh;
            let row = &mut self.w1[j * INPUTS..(j + 1) * INPUTS];
            for (w, xi) in row.iter_mut().zip(x.iter()) {
                *w -= lr * (dh * xi + 1e-4 * *w);
            }
        }
        self.b2 -= lr * err;
        y
    }

    /// Text form: a header line, then all the numbers.
    pub fn to_text(&self) -> String {
        let mut out = format!("stickman-net {INPUTS} {HIDDEN}\n");
        let line = |name: &str, v: &[f32]| {
            let nums: Vec<String> = v.iter().map(|x| format!("{x:.5}")).collect();
            format!("{name} {}\n", nums.join(" "))
        };
        out += &line("w1", &self.w1);
        out += &line("b1", &self.b1);
        out += &line("w2", &self.w2);
        out += &line("b2", &[self.b2]);
        out
    }

    pub fn parse(text: &str) -> Option<Net> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
        let mut head = lines.next()?.split_whitespace();
        if head.next()? != "stickman-net" {
            return None;
        }
        let inputs: usize = head.next()?.parse().ok()?;
        let hidden: usize = head.next()?.parse().ok()?;
        if inputs != INPUTS || hidden != HIDDEN {
            return None;
        }
        let mut net = Net { w1: Vec::new(), b1: Vec::new(), w2: Vec::new(), b2: 0.0 };
        for line in lines {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            let nums: Vec<f32> = parts.filter_map(|p| p.parse().ok()).collect();
            match name {
                "w1" => net.w1 = nums,
                "b1" => net.b1 = nums,
                "w2" => net.w2 = nums,
                "b2" => net.b2 = *nums.first()?,
                _ => {}
            }
        }
        let ok = net.w1.len() == INPUTS * HIDDEN
            && net.b1.len() == HIDDEN
            && net.w2.len() == HIDDEN
            && net.w1.iter().chain(&net.b1).chain(&net.w2).all(|v| v.is_finite());
        ok.then_some(net)
    }
}

/// One thing it did, the situation, and how it turned out.
pub struct Experience {
    pub kind: String,
    pub ctx: [f32; CTX],
    pub tags: [f32; TAGS],
    pub reward: f32,
    /// Praised or scolded by you.
    pub taught: bool,
}

/// Don't let the log grow without bound.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// Append an experience to `behaviour_log.jsonl` (only with "Keep a
/// training log" on), for `train.py finetune`.
pub fn log(e: &Experience) {
    let Some(dir) = dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("behaviour_log.jsonl");
    if std::fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("behaviour_log.old.jsonl"));
    }
    let nums = |v: &[f32]| v.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(",");
    let line = format!(
        "{{\"kind\":\"{}\",\"x\":[{}],\"a\":[{}],\"r\":{:.3},\"taught\":{}}}\n",
        e.kind,
        nums(&e.ctx),
        nums(&e.tags),
        e.reward,
        e.taught
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip() {
        let net = Net::blank();
        let back = Net::parse(&net.to_text()).expect("parse");
        let x = [0.5; INPUTS];
        assert!((net.predict(&x) - back.predict(&x)).abs() < 1e-3);
        assert!(Net::parse("stickman-net 3 2\nw1 1 2\n").is_none());
        assert!(Net::parse("not trained yet").is_none());
    }

    #[test]
    fn learns_from_experience() {
        let mut net = Net::blank();
        let mut good = [0.0; INPUTS];
        good[0] = 1.0;
        good[INPUTS - 1] = 1.0;
        let mut bad = [0.0; INPUTS];
        bad[1] = 1.0;
        bad[INPUTS - 1] = 1.0;
        for _ in 0..400 {
            net.learn(&good, 0.8, 0.05);
            net.learn(&bad, -0.8, 0.05);
        }
        assert!(net.predict(&good) > 0.5, "{}", net.predict(&good));
        assert!(net.predict(&bad) < -0.5, "{}", net.predict(&bad));
    }
}
