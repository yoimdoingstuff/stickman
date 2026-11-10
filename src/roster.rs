//! Saved stickmen: each one's name, colour, whether it was out on the
//! desktop, and its whole brain (personality, friends and enemies, what it
//! has learned), in `%APPDATA%\stickman\stickmen\<id>.txt`.

use std::path::PathBuf;

use crate::settings::{parse_hex, to_hex};

pub struct Saved {
    pub id: u32,
    pub name: String,
    pub color: [u8; 3],
    /// Was out on the desktop when the program closed (comes back next time).
    pub present: bool,
    /// `UtilityBrain::save` text.
    pub brain: String,
}

const NAMES: [&str; 24] = [
    "Inky", "Doodle", "Scribble", "Sketch", "Twig", "Noodle", "Pip", "Dash", "Scratch", "Smudge", "Quill", "Stick",
    "Blot", "Squiggle", "Flick", "Zigzag", "Nib", "Graphite", "Stroke", "Etch", "Loop", "Swoosh", "Dot", "Crayon",
];

fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("stickman").join("stickmen"))
}

pub fn load_all() -> Vec<Saved> {
    let Some(dir) = dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let Some(id) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let (head, brain) = text.split_once("\n---\n").unwrap_or((text.as_str(), ""));
        let mut saved = Saved { id, name: format!("Stickman {id}"), color: [0, 0, 0], present: true, brain: brain.to_string() };
        for line in head.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "name" => saved.name = v.trim().to_string(),
                "color" => saved.color = parse_hex(v).unwrap_or([0, 0, 0]),
                "present" => saved.present = v.trim() == "true",
                _ => {}
            }
        }
        out.push(saved);
    }
    out.sort_by_key(|s| s.id);
    out
}

pub fn save(s: &Saved) {
    let Some(dir) = dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let text = format!("name={}\ncolor={}\npresent={}\n---\n{}", s.name, to_hex(s.color), s.present, s.brain);
    // Write then rename, so a crash mid-save can't lose a stickman.
    let tmp = dir.join(format!("{}.tmp", s.id));
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(format!("{}.txt", s.id)));
    }
}

/// Gone for good.
pub fn forget(id: u32) {
    if let Some(dir) = dir() {
        let _ = std::fs::remove_file(dir.join(format!("{id}.txt")));
    }
}

/// An id no saved or current stickman uses.
pub fn new_id(taken: &[u32]) -> u32 {
    let saved: Vec<u32> = load_all().iter().map(|s| s.id).collect();
    (1..).find(|i| !taken.contains(i) && !saved.contains(i)).unwrap_or(1)
}

/// A name nobody has yet (falls back to numbering).
pub fn new_name(taken: &[String], pick: f32) -> String {
    let free: Vec<&str> = NAMES.iter().copied().filter(|n| !taken.iter().any(|t| t == n)).collect();
    if free.is_empty() {
        return format!("Stickman {}", taken.len() + 1);
    }
    free[((pick * free.len() as f32) as usize).min(free.len() - 1)].to_string()
}
