//! User settings, saved as a small text file in %APPDATA%\stickman.
//! Plain `key=value` lines so it's easy to read and edit by hand.

use std::path::PathBuf;

pub struct Settings {
    /// Body colour as RGB.
    pub color: [u8; 3],
    /// Draw a contrasting rim around the body.
    pub outline: bool,
    pub stay_in_front: bool,
    /// Allow it to jump up and hang off your mouse cursor.
    pub grab_cursor: bool,
    /// Gravity multiplier (1.0 = normal).
    pub gravity: f32,
    /// Stomp open a safe Minesweeper tile every few of your moves.
    pub minesweeper_help: bool,
    /// ...and sometimes pick a risky one (off by default: then it never
    /// makes you lose).
    pub minesweeper_mistakes: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            color: [0, 0, 0],
            outline: true,
            stay_in_front: false,
            grab_cursor: true,
            gravity: 1.0,
            minesweeper_help: true,
            minesweeper_mistakes: false,
        }
    }
}

fn path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("stickman").join("settings.txt"))
}

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

pub fn to_hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Settings::default();
        let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return s;
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let flag = value.trim() == "true";
            match key.trim() {
                "color" => {
                    if let Some(c) = parse_hex(value) {
                        s.color = c;
                    }
                }
                "outline" => s.outline = flag,
                "stay_in_front" => s.stay_in_front = flag,
                "grab_cursor" => s.grab_cursor = flag,
                "minesweeper_help" => s.minesweeper_help = flag,
                "minesweeper_mistakes" => s.minesweeper_mistakes = flag,
                "gravity" => {
                    if let Ok(g) = value.trim().parse::<f32>() {
                        s.gravity = g.clamp(0.1, 5.0);
                    }
                }
                _ => {}
            }
        }
        s
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = format!(
            "color={}\noutline={}\nstay_in_front={}\ngrab_cursor={}\ngravity={}\nminesweeper_help={}\nminesweeper_mistakes={}\n",
            to_hex(self.color),
            self.outline,
            self.stay_in_front,
            self.grab_cursor,
            self.gravity,
            self.minesweeper_help,
            self.minesweeper_mistakes
        );
        let _ = std::fs::write(p, text);
    }
}
