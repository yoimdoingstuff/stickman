//! Learning what you like.
//!
//! With "Learn what I like" turned on, every choice the stickman makes is
//! written to `%APPDATA%\stickman\behaviour_log.jsonl` along with the
//! situation it was in and how much you reacted in the next few seconds
//! (picking it up, moving your mouse over to it). Nothing leaves your PC.
//!
//! `training/train.py` turns that log into `brain_model.txt`: for each kind
//! of action, a tiny logistic-regression model of "how likely is this to get
//! a reaction in this situation". When that file exists the brain nudges its
//! choices towards what you react to.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Number of situation features (see `Brain` for what they are).
pub const FEATURES: usize = 10;
pub const FEATURE_NAMES: [&str; FEATURES] = [
    "boredom",
    "energy",
    "curiosity",
    "laziness",
    "mischief",
    "playfulness",
    "health",
    "inside_window",
    "cursor_near",
    "others_around",
];

/// How long after a choice reactions still count towards it.
const WINDOW: Duration = Duration::from_secs(8);
/// Don't let the log grow without bound.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("stickman"))
}

/// Per action kind: bias and one weight per feature.
pub struct Model {
    kinds: Vec<(String, f32, Vec<f32>)>,
}

impl Model {
    /// `brain_model.txt`, if it's been trained.
    pub fn load() -> Option<Model> {
        let text = std::fs::read_to_string(dir()?.join("brain_model.txt")).ok()?;
        Model::parse(&text)
    }

    /// Format: a line "features N", then "Kind bias w1 .. wN" per action.
    pub fn parse(text: &str) -> Option<Model> {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let header = lines.next()?;
        let n: usize = header.strip_prefix("features ")?.trim().parse().ok()?;
        if n != FEATURES {
            return None;
        }
        let mut kinds = Vec::new();
        for line in lines {
            let mut parts = line.split_whitespace();
            let Some(kind) = parts.next() else { continue };
            let nums: Vec<f32> = parts.filter_map(|p| p.parse().ok()).collect();
            if nums.len() == n + 1 {
                kinds.push((kind.to_string(), nums[0], nums[1..].to_vec()));
            }
        }
        (!kinds.is_empty()).then_some(Model { kinds })
    }

    /// How much to favour (or avoid) this action here: about ±0.3.
    pub fn bonus(&self, kind: &str, x: &[f32; FEATURES]) -> f32 {
        let Some((_, b, w)) = self.kinds.iter().find(|(k, _, _)| k == kind) else { return 0.0 };
        let z = b + w.iter().zip(x.iter()).map(|(w, x)| w * x).sum::<f32>();
        let p = 1.0 / (1.0 + (-z).exp());
        (p - 0.5) * 0.6
    }
}

struct Pending {
    kind: String,
    x: [f32; FEATURES],
    at: Instant,
    engagement: f32,
}

/// Collects choices and your reactions to them, and writes them to the log.
#[derive(Default)]
pub struct Logger {
    pending: Option<Pending>,
}

impl Logger {
    /// The stickman just decided to do `kind` in situation `x`.
    pub fn chose(&mut self, kind: String, x: [f32; FEATURES]) {
        self.flush();
        self.pending = Some(Pending { kind, x, at: Instant::now(), engagement: 0.0 });
    }

    /// You reacted (picked it up = 1, came over with the mouse = 0.3...).
    pub fn engaged(&mut self, amount: f32) {
        if let Some(p) = &mut self.pending {
            if p.at.elapsed() < WINDOW {
                p.engagement = (p.engagement + amount).min(1.0);
            }
        }
    }

    /// Call now and then: writes out choices whose time is up.
    pub fn tick(&mut self) {
        if self.pending.as_ref().is_some_and(|p| p.at.elapsed() >= WINDOW) {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let Some(p) = self.pending.take() else { return };
        let Some(dir) = dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("behaviour_log.jsonl");
        if std::fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
            // Keep the newer half-ish: start a fresh file, keep the old one.
            let _ = std::fs::rename(&path, dir.join("behaviour_log.old.jsonl"));
        }
        let xs: Vec<String> = p.x.iter().map(|v| format!("{v:.3}")).collect();
        let line = format!("{{\"kind\":\"{}\",\"x\":[{}],\"engagement\":{:.2}}}\n", p.kind, xs.join(","), p.engagement);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_round_trip() {
        let text = "features 10\nJump 0.5 1 0 0 0 0 0 0 0 0 0\nSleep -2 0 0 0 0 0 0 0 0 0 0\n";
        let m = Model::parse(text).expect("model");
        let bored = [1.0, 0.5, 0.5, 0.5, 0.5, 0.5, 1.0, 0.0, 0.0, 0.0];
        // Liked: positive nudge. Disliked: negative. Unknown: none.
        assert!(m.bonus("Jump", &bored) > 0.1);
        assert!(m.bonus("Sleep", &bored) < -0.1);
        assert_eq!(m.bonus("Dance", &bored), 0.0);
    }

    #[test]
    fn rejects_wrong_feature_count() {
        assert!(Model::parse("features 3\nJump 0 1 2 3\n").is_none());
    }
}
