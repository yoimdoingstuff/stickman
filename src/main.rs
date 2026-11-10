// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod brain;
mod games;
mod instance;
mod learn;
mod math;
mod platform;
mod ragdoll;
mod render;
mod rig;
mod rng;
mod roster;
mod settings;
mod stickman;
mod world;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use brain::{Brain, BrainEvent, PeerView, Personality, UtilityBrain};
use games::minesweeper::{Helper, Look};
use games::{Game, GameEvent, Watcher};
use math::{v2, Rect, Vec2};
use platform::{Event, MenuItem, Platform, SettingItem};
use render::Style;
use rng::Rng;
use settings::Settings;
use stickman::{Dims, Mode, Stickman};
use world::{button_matches, icon_key, ledge_key, page_ledges, Page, PeerBody, Support, WinId, World};

/// Fixed physics/animation step.
const STEP: f32 = 1.0 / 120.0;
/// How often to re-scan all windows (the ones being moved are polled every frame).
const WINDOW_SCAN: f32 = 0.15;
/// How often to re-read the taskbar's buttons.
const DOOR_SCAN: f32 = 2.0;
/// How often everyone is saved (they're also saved when the program closes).
const AUTOSAVE: f32 = 120.0;
/// Base size multiplier on top of the monitor's DPI scale.
const SIZE: f32 = 1.15;

const MENU_FRONT: u32 = 1;
const MENU_PAUSE: u32 = 2;
const MENU_COLOR: u32 = 4;
const MENU_OUTLINE: u32 = 6;
const MENU_CURSOR: u32 = 7;
const MENU_MS_HELP: u32 = 8;
const MENU_MS_MISTAKES: u32 = 9;
const MENU_MS_SNAPSHOT: u32 = 10;
const MENU_SETTINGS: u32 = 11;
const MENU_ICONS: u32 = 12;
const MENU_WEB: u32 = 13;
const MENU_FOLDER: u32 = 14;
const MENU_MOVE_ICONS: u32 = 15;
const MENU_LEARN: u32 = 16;
const MENU_ADD: u32 = 17;
/// Gravity dropdown (value = index into GRAVITY_PRESETS).
const MENU_GRAVITY: u32 = 20;
const GRAVITY_PRESETS: [(&str, f32); 5] = [
    ("Moon (0.3x)", 0.3),
    ("Low (0.6x)", 0.6),
    ("Normal", 1.0),
    ("Heavy (1.6x)", 1.6),
    ("Jupiter (2.5x)", 2.5),
];
const MENU_QUIT: u32 = 99;
/// One stickman's menu: MENU_BUDDY + slot * 10 + one of the B_ codes.
const MENU_BUDDY: u32 = 1000;
const B_LIMP: u32 = 0;
const B_COME: u32 = 1;
const B_COLOR: u32 = 2;
const B_GOOD: u32 = 3;
const B_BAD: u32 = 4;
const B_HOME: u32 = 5;
const B_FORGET: u32 = 6;
/// "Bring back": MENU_BRING + index into the stickmen who are at home.
const MENU_BRING: u32 = 5000;

/// Browsers, and apps built on a browser engine (Discord, Spotify, Slack...),
/// whose contents it can explore (they all expose their page's boxes
/// through UI Automation). Other apps work too once it's inside them.
fn has_web_content(exe: &str) -> bool {
    matches!(
        exe,
        "chrome.exe"
            | "msedge.exe"
            | "firefox.exe"
            | "brave.exe"
            | "opera.exe"
            | "vivaldi.exe"
            | "librewolf.exe"
            | "waterfox.exe"
            | "floorp.exe"
            | "zen.exe"
            | "arc.exe"
            | "chromium.exe"
            | "iexplore.exe"
            | "discord.exe"
            | "spotify.exe"
            | "slack.exe"
            | "ms-teams.exe"
            | "teams.exe"
            | "whatsapp.exe"
            | "obsidian.exe"
            | "notion.exe"
            | "code.exe"
    )
}

/// A desktop icon flying through the air after being kicked.
struct IconFlight {
    index: usize,
    key: u32,
    /// Top-left corner.
    pos: Vec2,
    vel: Vec2,
    size: Vec2,
    age: f32,
}

impl IconFlight {
    /// Move it along; false once it has come to rest.
    fn step(&mut self, dt: f32, g: f32, world: &World) -> bool {
        self.age += dt;
        self.vel.y += g * dt;
        self.pos += self.vel * dt;
        let w = world.work;
        if self.pos.x < w.l {
            self.pos.x = w.l;
            self.vel.x = self.vel.x.abs() * 0.5;
        }
        if self.pos.x + self.size.x > w.r {
            self.pos.x = w.r - self.size.x;
            self.vel.x = -self.vel.x.abs() * 0.5;
        }
        self.pos.y = self.pos.y.max(w.t);
        let floor = world.floor_at(self.pos.x + self.size.x * 0.5).map(|f| f.y).unwrap_or(w.b);
        if self.pos.y + self.size.y > floor {
            self.pos.y = floor - self.size.y;
            self.vel.y = -self.vel.y.abs() * 0.3;
            self.vel.x *= 0.6;
        }
        self.age < 4.0 && self.vel.len() > 30.0
    }
}

/// One stickman out on the desktop.
struct Buddy {
    id: u32,
    name: String,
    color: [u8; 3],
    style: Style,
    man: Stickman,
    brain: UtilityBrain,
    /// Its overlay window.
    slot: usize,
    /// How much you've been paying attention to it lately (0..1).
    attention: f32,
    /// Being stroked with the mouse.
    pet: f32,
    body_hit_cooldown: f32,
}

impl Buddy {
    fn saved(&self, present: bool) -> roster::Saved {
        roster::Saved { id: self.id, name: self.name.clone(), color: self.color, present, brain: self.brain.save() }
    }

    /// Its own menu (right-click, or its entry in the tray menu).
    fn menu(&self) -> Vec<MenuItem> {
        let id = |code: u32| MENU_BUDDY + self.slot as u32 * 10 + code;
        vec![
            MenuItem::new(id(B_LIMP), "Ragdoll", false),
            MenuItem::new(id(B_COME), "Come here", false),
            MenuItem::new(id(B_COLOR), "Change colour...", false),
            MenuItem::separator(),
            MenuItem::new(id(B_GOOD), "Good job!", false),
            MenuItem::new(id(B_BAD), "Don't do that!", false),
            MenuItem::separator(),
            MenuItem::new(id(B_HOME), "Send home (it's saved)", false),
        ]
    }
}

/// Put a stickman on the desktop.
#[allow(clippy::too_many_arguments)]
fn spawn(
    plat: &mut dyn Platform,
    d: Dims,
    world: &World,
    settings: &Settings,
    rng: &mut Rng,
    id: u32,
    name: String,
    color: [u8; 3],
    brain: UtilityBrain,
) -> Buddy {
    let s = d.s;
    let at = v2(world.work.l + world.work.width() * rng.range(0.15, 0.85), world.work.t + 80.0 * s);
    let mut man = Stickman::new(d, at);
    man.force_front = settings.stay_in_front;
    man.allow_cursor = settings.grab_cursor;
    man.gravity = settings.gravity;
    man.move_icons = settings.move_icons;
    Buddy {
        id,
        name,
        color,
        style: Style::new(color, settings.outline),
        man,
        brain,
        slot: plat.create_overlay(),
        attention: 0.0,
        pet: 0.0,
        body_hit_cooldown: 0.0,
    }
}

/// A brand-new stickman with a random personality and a fresh copy of the
/// base brain.
fn spawn_new(
    plat: &mut dyn Platform,
    d: Dims,
    world: &World,
    settings: &Settings,
    rng: &mut Rng,
    buddies: &[Buddy],
    home: &[roster::Saved],
) -> Buddy {
    let ids: Vec<u32> = buddies.iter().map(|b| b.id).chain(home.iter().map(|h| h.id)).collect();
    let names: Vec<String> = buddies.iter().map(|b| b.name.clone()).chain(home.iter().map(|h| h.name.clone())).collect();
    let id = roster::new_id(&ids);
    let name = roster::new_name(&names, rng.f32());
    let brain = UtilityBrain::new(id, Personality::random(rng));
    spawn(plat, d, world, settings, rng, id, name, settings.color, brain)
}

/// A saved stickman coming back, brain and all.
fn spawn_saved(
    plat: &mut dyn Platform,
    d: Dims,
    world: &World,
    settings: &Settings,
    rng: &mut Rng,
    saved: roster::Saved,
) -> Buddy {
    let brain = UtilityBrain::load(saved.id, &saved.brain).unwrap_or_else(|| UtilityBrain::new(saved.id, Personality::random(rng)));
    spawn(plat, d, world, settings, rng, saved.id, saved.name, saved.color, brain)
}

/// The rectangle covering all of these.
fn union(rects: &[Rect]) -> Rect {
    rects.iter().fold(rects.first().copied().unwrap_or(Rect { l: 0.0, t: 0.0, r: 1.0, b: 1.0 }), |a, r| Rect {
        l: a.l.min(r.l),
        t: a.t.min(r.t),
        r: a.r.max(r.r),
        b: a.b.max(r.b),
    })
}

/// What the settings panel shows.
fn settings_items(settings: &Settings, paused: bool) -> Vec<SettingItem> {
    let check = |id: u32, label: &str, on: bool| SettingItem::Check { id, label: label.to_string(), on };
    let button = |id: u32, label: &str| SettingItem::Button { id, label: label.to_string() };
    let heading = |t: &str| SettingItem::Heading(t.to_string());
    let gravity = GRAVITY_PRESETS.iter().position(|(_, g)| (settings.gravity - g).abs() < 0.01).unwrap_or(2);
    vec![
        heading("Looks"),
        button(MENU_COLOR, "Colour for new stickmen..."),
        check(MENU_OUTLINE, "Show outline", settings.outline),
        heading("Behaviour"),
        check(MENU_FRONT, "Always stay in front of windows", settings.stay_in_front),
        check(MENU_CURSOR, "Can grab my mouse cursor", settings.grab_cursor),
        check(MENU_ICONS, "Play on my desktop icons", settings.desktop_icons),
        check(MENU_WEB, "Explore web pages and apps", settings.web_pages),
        check(MENU_MOVE_ICONS, "Can kick my desktop icons around (really moves them)", settings.move_icons),
        check(MENU_PAUSE, "Pause everyone's thinking", paused),
        SettingItem::Choice {
            id: MENU_GRAVITY,
            label: "Gravity".into(),
            options: GRAVITY_PRESETS.iter().map(|(l, _)| l.to_string()).collect(),
            selected: gravity,
        },
        heading("Games"),
        check(MENU_MS_HELP, "Help me with Minesweeper", settings.minesweeper_help),
        check(MENU_MS_MISTAKES, "Let it make mistakes (can lose you the game)", settings.minesweeper_mistakes),
        button(MENU_MS_SNAPSHOT, "Save Minesweeper snapshot"),
        heading("Other"),
        button(MENU_ADD, "Add a stickman"),
        check(MENU_LEARN, "Keep a training log (for training/train.py)", settings.learn),
        button(MENU_FOLDER, "Open settings folder"),
    ]
}

fn look_of(game: Game) -> Look {
    if game == Game::MinesweeperXp {
        Look::Xp
    } else {
        Look::Modern
    }
}

fn crash_log_path() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join("stickman").join("crash.txt"))
}

/// Write any crash (panic) to %APPDATA%\stickman\crash.txt, so it doesn't
/// just vanish without a trace.
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = crash_log_path() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let when = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let text = format!("stickman {} crashed (unix time {when}):\n{info}\n\n", env!("CARGO_PKG_VERSION"));
            let old = std::fs::read_to_string(&path).unwrap_or_default();
            // Keep the file small: newest crash first, a few kept.
            let keep: String = old.chars().take(8000).collect();
            let _ = std::fs::write(&path, text + &keep);
        }
        default(info);
    }));
}

/// Run game-related code; if it panics, note it and carry on without it
/// (the stickman shouldn't die because a game confused it).
fn guarded<T>(enabled: &mut bool, f: impl FnOnce() -> T) -> Option<T> {
    if !*enabled {
        return None;
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(_) => {
            *enabled = false;
            None
        }
    }
}

/// The tray icon, in the colour new stickmen get.
fn update_tray(plat: &mut dyn Platform, settings: &Settings, count: usize) {
    let size = 32;
    if let Some(icon) = render::icon(size, settings.color) {
        let tip = match count {
            0 => "Stickman (everyone's at home)".to_string(),
            1 => "Stickman: 1 out".to_string(),
            n => format!("Stickman: {n} out"),
        };
        plat.set_tray(icon.data(), size, &tip);
    }
}

fn main() {
    install_crash_log();
    // One program for all the stickmen: if one is already running, it adds
    // a stickman for us and we're done.
    let host = match instance::start() {
        instance::Start::Forwarded => return,
        instance::Start::Host(h) => Some(h),
        instance::Start::Alone => None,
    };
    let mut plat = match platform::create() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("stickman: {e}");
            return;
        }
    };
    let d = Dims::new(plat.dpi_scale() * SIZE);
    let s = d.s;
    let mut rng = Rng::seeded();
    let screens = plat.screens();
    let mut world = World {
        work: union(&screens),
        primary: plat.work_area(),
        screens,
        windows: plat.windows(),
        taskbar: plat.taskbar(),
        doors: Vec::new(),
        peers: Vec::new(),
        pages: Vec::new(),
        icons: Vec::new(),
    };
    let mut settings = Settings::load();

    // Everyone who was out last time comes back; the rest stay at home
    // (they can be brought back from the tray menu).
    let mut buddies: Vec<Buddy> = Vec::new();
    let mut home: Vec<roster::Saved> = Vec::new();
    for saved in roster::load_all() {
        if saved.present {
            buddies.push(spawn_saved(&mut *plat, d, &world, &settings, &mut rng, saved));
        } else {
            home.push(saved);
        }
    }
    if buddies.is_empty() {
        let b = spawn_new(&mut *plat, d, &world, &settings, &mut rng, &buddies, &home);
        buddies.push(b);
    }
    update_tray(&mut *plat, &settings, buddies.len());
    let mut tray_count = buddies.len();

    // Each window's program (looked up once per window).
    let mut exes: HashMap<WinId, String> = HashMap::new();
    let mut page_scan = 0.0f32;
    // The page box being followed closely in each window: (its key, when asked).
    let mut tracking: HashMap<WinId, (u32, Instant)> = HashMap::new();
    // Desktop icons in their real order (index = what Explorer calls them),
    // and icons currently flying after being kicked.
    let mut icon_list: Vec<(String, Rect)> = Vec::new();
    let mut flights: Vec<IconFlight> = Vec::new();
    let mut icon_scan = 0.0f32;
    let mut watcher = Watcher::default();
    let mut helper = Helper::new(&mut rng);
    // Turned off for the rest of the session if the game code ever panics.
    let mut games_ok = true;
    // Which stickman is helping with Minesweeper.
    let mut sweeper_id: Option<u32> = None;

    let size = ((190.0 * s).ceil() as u32).max(64);
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("pixmap");

    let mut paused = false;
    let mut last = Instant::now();
    let mut acc = 0.0f32;
    let mut scan = 0.0f32;
    let mut door_scan = 0.0f32;
    let mut topmost = 0.0f32;
    let mut autosave = AUTOSAVE;
    let mut last_cursor = plat.cursor();

    'run: loop {
        let frame_start = Instant::now();

        for ev in plat.poll_events() {
            // A command from a menu (value -1 = toggle) or the settings
            // panel (value = new checkbox state / dropdown index).
            let (id, value) = match ev {
                Event::Quit => break 'run,
                Event::MouseDown { slot } => {
                    let c = plat.cursor();
                    if let Some(b) = buddies.iter_mut().find(|b| b.slot == slot) {
                        b.man.grab(c);
                        b.brain.notify(BrainEvent::Grabbed);
                        b.attention = (b.attention + 0.5).min(1.0);
                    }
                    continue;
                }
                Event::MouseUp { slot } => {
                    if let Some(b) = buddies.iter_mut().find(|b| b.slot == slot) {
                        if b.man.release(&world) {
                            b.brain.notify(BrainEvent::Thrown);
                        }
                    }
                    continue;
                }
                Event::RightClick { slot } => {
                    let Some(b) = buddies.iter().find(|b| b.slot == slot) else { continue };
                    let mut items = b.menu();
                    items.extend([
                        MenuItem::separator(),
                        MenuItem::new(MENU_SETTINGS, "Settings...", false),
                        MenuItem::new(MENU_QUIT, "Quit (everyone's saved)", false),
                    ]);
                    let picked = plat.show_menu(&items);
                    // The menu blocks; don't simulate the time it was open.
                    last = Instant::now();
                    match picked {
                        Some(id) => (id, -1),
                        None => continue,
                    }
                }
                Event::Tray => {
                    // Everyone, in one place.
                    let mut items: Vec<MenuItem> = buddies
                        .iter()
                        .map(|b| {
                            let mut sub = b.menu();
                            sub.push(MenuItem::new(MENU_BUDDY + b.slot as u32 * 10 + B_FORGET, "Forget forever...", false));
                            MenuItem::submenu(&b.name, sub)
                        })
                        .collect();
                    items.push(MenuItem::new(MENU_ADD, "Add a stickman", false));
                    if !home.is_empty() {
                        let back = home
                            .iter()
                            .enumerate()
                            .map(|(i, h)| MenuItem::new(MENU_BRING + i as u32, &h.name, false))
                            .collect();
                        items.push(MenuItem::submenu("Bring back", back));
                    }
                    items.extend([
                        MenuItem::separator(),
                        MenuItem::new(MENU_PAUSE, "Pause everyone", paused),
                        MenuItem::new(MENU_SETTINGS, "Settings...", false),
                        MenuItem::separator(),
                        MenuItem::new(MENU_QUIT, "Quit (everyone's saved)", false),
                    ]);
                    let picked = plat.show_menu(&items);
                    last = Instant::now();
                    match picked {
                        Some(id) => (id, -1),
                        None => continue,
                    }
                }
                Event::Setting { id, value } => (id, value),
            };
            let flag = |current: bool| if value < 0 { !current } else { value != 0 };

            // One stickman's own commands.
            if (MENU_BUDDY..MENU_BRING).contains(&id) {
                let (slot, code) = (((id - MENU_BUDDY) / 10) as usize, (id - MENU_BUDDY) % 10);
                let Some(i) = buddies.iter().position(|b| b.slot == slot) else { continue };
                match code {
                    B_LIMP => buddies[i].man.go_limp(),
                    B_COME => {
                        let c = plat.cursor();
                        buddies[i].man.teleport(c - v2(0.0, 40.0 * s));
                    }
                    B_COLOR => {
                        if let Some(c) = plat.pick_color(buddies[i].color) {
                            buddies[i].color = c;
                            buddies[i].style = Style::new(c, settings.outline);
                            roster::save(&buddies[i].saved(true));
                        }
                        last = Instant::now();
                    }
                    B_GOOD | B_BAD => {
                        // Teaching: what it's doing right now gets the credit
                        // (or the blame), and counts extra.
                        let v = if code == B_GOOD { 1.0 } else { -1.0 };
                        buddies[i].brain.notify(BrainEvent::Teach(v));
                        buddies[i].man.interrupt();
                    }
                    B_HOME => {
                        let b = buddies.remove(i);
                        let saved = b.saved(false);
                        roster::save(&saved);
                        home.push(saved);
                        plat.destroy_overlay(b.slot);
                    }
                    B_FORGET => {
                        let text = format!("Forget {} forever? Everything it has learned will be lost.", buddies[i].name);
                        if plat.confirm(&text) {
                            let b = buddies.remove(i);
                            roster::forget(b.id);
                            plat.destroy_overlay(b.slot);
                            for other in buddies.iter_mut() {
                                other.brain.forget(b.id);
                            }
                        }
                        last = Instant::now();
                    }
                    _ => {}
                }
                continue;
            }
            if id >= MENU_BRING {
                let i = (id - MENU_BRING) as usize;
                if i < home.len() {
                    let mut saved = home.remove(i);
                    saved.present = true;
                    roster::save(&saved);
                    let b = spawn_saved(&mut *plat, d, &world, &settings, &mut rng, saved);
                    buddies.push(b);
                }
                continue;
            }
            match id {
                MENU_SETTINGS => plat.open_settings(&settings_items(&settings, paused)),
                MENU_ADD => {
                    let b = spawn_new(&mut *plat, d, &world, &settings, &mut rng, &buddies, &home);
                    roster::save(&b.saved(true));
                    buddies.push(b);
                }
                MENU_FRONT => {
                    settings.stay_in_front = flag(settings.stay_in_front);
                    for b in buddies.iter_mut() {
                        b.man.force_front = settings.stay_in_front;
                    }
                    settings.save();
                }
                MENU_CURSOR => {
                    settings.grab_cursor = flag(settings.grab_cursor);
                    for b in buddies.iter_mut() {
                        b.man.allow_cursor = settings.grab_cursor;
                    }
                    settings.save();
                }
                MENU_ICONS => {
                    settings.desktop_icons = flag(settings.desktop_icons);
                    settings.save();
                }
                MENU_WEB => {
                    settings.web_pages = flag(settings.web_pages);
                    settings.save();
                }
                MENU_MOVE_ICONS => {
                    settings.move_icons = flag(settings.move_icons);
                    for b in buddies.iter_mut() {
                        b.man.move_icons = settings.move_icons;
                    }
                    settings.save();
                }
                MENU_LEARN => {
                    settings.learn = flag(settings.learn);
                    settings.save();
                }
                MENU_PAUSE => paused = flag(paused),
                MENU_COLOR => {
                    if let Some(c) = plat.pick_color(settings.color) {
                        settings.color = c;
                        settings.save();
                        update_tray(&mut *plat, &settings, buddies.len());
                    }
                    last = Instant::now();
                }
                MENU_OUTLINE => {
                    settings.outline = flag(settings.outline);
                    for b in buddies.iter_mut() {
                        b.style = Style::new(b.color, settings.outline);
                    }
                    settings.save();
                }
                MENU_MS_HELP => {
                    settings.minesweeper_help = flag(settings.minesweeper_help);
                    settings.save();
                }
                MENU_MS_MISTAKES => {
                    settings.minesweeper_mistakes = flag(settings.minesweeper_mistakes);
                    settings.save();
                }
                MENU_MS_SNAPSHOT => {
                    if let Some((id, game)) = watcher.find(Game::is_minesweeper) {
                        // Saves minesweeper.png + minesweeper.txt, then shows the folder.
                        let mut ok = true;
                        match guarded(&mut ok, || helper.snapshot(&mut *plat, id, look_of(game))) {
                            Some(Ok(dir)) => {
                                let _ = std::process::Command::new("explorer").arg(dir).spawn();
                            }
                            Some(Err(e)) => eprintln!("stickman: {e}"),
                            None => {}
                        }
                    }
                }
                MENU_FOLDER => {
                    if let Some(dir) = crash_log_path().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::process::Command::new("explorer").arg(dir).spawn();
                    }
                }
                MENU_GRAVITY => {
                    if let Some((_, g)) = GRAVITY_PRESETS.get(value.max(0) as usize) {
                        settings.gravity = *g;
                        for b in buddies.iter_mut() {
                            b.man.gravity = settings.gravity;
                        }
                        settings.save();
                    }
                }
                MENU_QUIT => break 'run,
                _ => {}
            }
        }

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32().min(0.1);
        last = now;

        // Someone ran stickman.exe again: one more stickman.
        if let Some(h) = &host {
            for _ in 0..h.poll().min(4) {
                let b = spawn_new(&mut *plat, d, &world, &settings, &mut rng, &buddies, &home);
                roster::save(&b.saved(true));
                buddies.push(b);
            }
        }
        if tray_count != buddies.len() {
            tray_count = buddies.len();
            update_tray(&mut *plat, &settings, tray_count);
        }

        // Desktop icons to climb and swing on (read in the background).
        if settings.desktop_icons {
            icon_scan -= dt;
            if icon_scan <= 0.0 {
                icon_scan = 2.0;
                icon_list = plat.desktop_icons();
                world.icons = icon_list.iter().map(|(name, r)| (icon_key(name), *r)).collect();
            }
        } else {
            world.icons.clear();
        }

        // Remember where every window was, so moves are never missed (even on
        // the frames where the whole list is re-scanned).
        let before: Vec<(WinId, Rect)> = world.windows.iter().map(|w| (w.id, w.rect)).collect();
        scan -= dt;
        if scan <= 0.0 {
            world.windows = plat.windows();
            world.primary = plat.work_area();
            world.screens = plat.screens();
            world.work = union(&world.screens);
            world.taskbar = plat.taskbar();
            scan = WINDOW_SCAN;
            // Games opening, being won or lost.
            let events = guarded(&mut games_ok, || watcher.update(&*plat, &world)).unwrap_or_default();
            for e in events {
                for b in buddies.iter_mut() {
                    if matches!(e, GameEvent::Finished(..)) {
                        b.man.interrupt();
                    }
                    b.brain.notify(BrainEvent::Game(e));
                }
            }
        } else {
            // The windows they're attached to, and the one in front (it's
            // the one being dragged around), are tracked every frame.
            let mut live: Vec<WinId> = buddies.iter().filter_map(|b| b.man.attached_window()).collect();
            live.extend(world.windows.first().map(|w| w.id));
            live.sort_unstable();
            live.dedup();
            for id in live {
                world.set_window_rect(id, plat.window_rect(id));
            }
        }
        // Any window that moved can shove or squash them.
        for (id, old) in before {
            if let Some(new) = world.window(id).map(|w| w.rect) {
                if new != old {
                    for b in buddies.iter_mut() {
                        if Some(id) != b.man.attached_window() {
                            b.man.shove(&world, id, old, new, dt);
                        }
                    }
                }
            }
        }

        // Which taskbar button leads to which window (they only ever dive
        // into a real app button, for the window they're heading to).
        door_scan -= dt;
        if door_scan <= 0.0 {
            door_scan = DOOR_SCAN;
            let buttons = plat.taskbar_buttons();
            let doors: Vec<(WinId, Rect)> = world
                .windows
                .iter()
                .filter_map(|w| {
                    let title = plat.window_title(w.id);
                    let exe = plat.window_process(w.id).map(|p| p.1).unwrap_or_default();
                    buttons
                        .iter()
                        .map(|(name, r)| (button_matches(name, &title, &exe), *r))
                        .filter(|(score, _)| *score > 0)
                        .max_by_key(|(score, _)| *score)
                        .map(|(_, r)| (w.id, r))
                })
                .collect();
            world.doors = doors;
        }

        let cursor = plat.cursor();
        let cursor_vel = if dt > 0.0 { (cursor - last_cursor) / dt } else { Vec2::ZERO };
        last_cursor = cursor;
        let foreground = plat.foreground();
        let focus_game = foreground.filter(|id| watcher.game_window(*id).is_some());

        // Pages: the boxes on the web page (or in the app) someone is inside,
        // and in the browser you're using, become ledges to stand on.
        page_scan -= dt;
        if !settings.web_pages {
            world.pages.clear();
            for (id, _) in tracking.drain() {
                plat.track(id, None);
            }
        } else if page_scan <= 0.0 {
            page_scan = 0.4;
            exes.retain(|id, _| world.window(*id).is_some());
            for w in &world.windows {
                exes.entry(w.id).or_insert_with(|| plat.window_process(w.id).map(|p| p.1).unwrap_or_default());
            }
            // Whatever window anyone is inside (any app), plus the browser in front.
            let mut wanted: Vec<WinId> = buddies.iter().filter_map(|b| b.man.room()).collect();
            if let Some(id) = foreground {
                if exes.get(&id).is_some_and(|e| has_web_content(e)) {
                    wanted.push(id);
                }
            }
            wanted.sort_unstable();
            wanted.dedup();
            let mut pages = Vec::new();
            for id in wanted {
                let current = world.pages.iter().find(|p| p.win == id).cloned();
                let page = match plat.page_boxes(id) {
                    // A newer read: lay the page out afresh.
                    Some(snap) if current.as_ref().map_or(true, |c| snap.taken > c.taken) => Some(Page {
                        win: id,
                        doc: snap.doc,
                        ledges: page_ledges(snap.doc, &snap.boxes, s),
                        taken: snap.taken,
                        probe: snap.probe,
                        shift: 0.0,
                    }),
                    _ => current,
                };
                pages.extend(page);
            }
            world.pages = pages;
            // Stop following boxes on pages nobody's on any more.
            let gone: Vec<WinId> = tracking.keys().copied().filter(|id| !world.pages.iter().any(|p| p.win == *id)).collect();
            for id in gone {
                tracking.remove(&id);
                plat.track(id, None);
            }
        }
        // Scrolling: the box being followed closely tells us exactly how far
        // the page has moved, whatever moved it (wheel, touchpad, scrollbar,
        // keys). Only real movement of the boxes counts, so it never floats
        // off when something that doesn't scroll is under it.
        for page in world.pages.iter_mut() {
            if let (Some((g0, top0)), Some((g, top, at))) = (page.probe, plat.page_probe(page.win)) {
                if g == g0 && at > page.taken && (top - top0).abs() >= 0.5 {
                    page.scroll_by(top - top0);
                    page.probe = Some((g0, top));
                }
            }
        }
        // Follow the box each stickman is standing on (one per page).
        for page in &world.pages {
            let standing = buddies.iter().find_map(|b| match b.man.mode {
                Mode::Ground(Support::Ledge { win, key }) if win == page.win => {
                    world.ledge(win, key, Some(b.man.pos.y + b.man.d.stance))
                }
                _ => None,
            });
            let Some(r) = standing else { continue };
            let key = ledge_key(&r);
            let stale = match tracking.get(&page.win) {
                Some((k, asked)) => *k != key || (asked.elapsed() > Duration::from_millis(700) && page.probe.is_none()),
                None => true,
            };
            if stale {
                // In the coordinates of the last read.
                let at = Rect { t: r.t - page.shift, b: r.b - page.shift, ..r };
                plat.track(page.win, Some(at));
                tracking.insert(page.win, (key, Instant::now()));
            }
        }

        // Everyone's bodies this frame: (id, box, ragdoll, hip, sleeping).
        let bodies: Vec<(u32, Rect, bool, Vec2, bool)> =
            buddies.iter().map(|b| (b.id, b.man.body_rect(), b.man.is_ragdoll(), b.man.center(), b.man.is_sleeping())).collect();

        // Thrown or flying fast into another stickman bowls it over (and the
        // hit slows the thrown one down).
        let mut hits: Vec<(u32, u32, Vec2)> = Vec::new();
        for b in buddies.iter_mut() {
            b.body_hit_cooldown -= dt;
            let flying = b.man.is_ragdoll() || b.man.mode == Mode::Air;
            if flying && b.body_hit_cooldown <= 0.0 && b.man.vel.len() > 600.0 * s {
                let mine = b.man.body_rect();
                let struck: Vec<u32> =
                    bodies.iter().filter(|o| o.0 != b.id && !o.2 && o.1.overlaps(&mine)).map(|o| o.0).collect();
                if !struck.is_empty() {
                    for to in struck {
                        hits.push((b.id, to, b.man.vel * 0.55));
                    }
                    b.man.bounce(b.man.vel * -0.35);
                    b.body_hit_cooldown = 0.5;
                }
            }
            // A random impulse takes over whatever it's doing (but lets it sleep).
            if b.brain.whim_due() && !b.man.is_sleeping() {
                b.man.interrupt();
            }
        }

        // Helping with Minesweeper while you play it (one of them does it).
        let mut sweeper: Option<(u32, Look)> = None;
        if settings.minesweeper_help && !paused && games_ok {
            if let Some((id, game)) = watcher.find(Game::is_minesweeper) {
                if foreground == Some(id) && !buddies.is_empty() {
                    let look = look_of(game);
                    let who = sweeper_id
                        .and_then(|w| buddies.iter().position(|b| b.id == w))
                        .or_else(|| buddies.iter().position(|b| b.man.can_help()))
                        .unwrap_or(0);
                    let b = &mut buddies[who];
                    sweeper_id = Some(b.id);
                    sweeper = Some((b.id, look));
                    let ready = b.man.can_help();
                    let mistakes = settings.minesweeper_mistakes;
                    let target =
                        guarded(&mut games_ok, || helper.update(dt, &mut *plat, id, look, ready, mistakes, &mut rng)).flatten();
                    if let Some(at) = target {
                        if !b.man.stomp(at, id) {
                            helper.cancel();
                        }
                    }
                }
            }
        }

        acc = (acc + dt).min(0.25);
        while acc >= STEP {
            acc -= STEP;
            let mut pokes: Vec<(u32, u32)> = Vec::new();
            let mut fives: Vec<(u32, u32)> = Vec::new();
            for i in 0..buddies.len() {
                let me = buddies[i].id;
                // Everyone else, as this one sees them.
                world.peers = bodies.iter().filter(|o| o.0 != me).map(|o| PeerBody { id: o.0, rect: o.1, ragdoll: o.2 }).collect();
                let b = &mut buddies[i];
                if !paused && b.man.wants_intent() {
                    let views = bodies
                        .iter()
                        .filter(|o| o.0 != me)
                        .map(|o| PeerView { id: o.0, hip: o.3, ragdoll: o.2, sleeping: o.4, ground: o.1.b })
                        .collect();
                    if let Some(mut p) = b.man.perceive(cursor, views, focus_game) {
                        let span = (world.primary.b - world.primary.t).max(1.0);
                        p.height = ((world.primary.b - p.foot.y) / span).clamp(0.0, 1.0);
                        p.attention = b.attention;
                        let intent = b.brain.decide(&p, &world, &mut rng);
                        b.man.apply(intent, &world);
                        if let Some(e) = b.brain.take_experience() {
                            if settings.learn {
                                learn::log(&e);
                            }
                        }
                    }
                }
                b.brain.tick(STEP);
                let ev = b.man.update(STEP, &world, cursor, cursor_vel);
                if let Some(impact) = ev.landed {
                    b.brain.notify(BrainEvent::Landed(impact));
                }
                if let Some((target, impulse, range)) = ev.hit {
                    if let Some(o) = bodies.iter().find(|o| o.0 == target) {
                        if (o.3 - b.man.pos).len() < range {
                            hits.push((me, target, impulse));
                        }
                    }
                }
                // Kicked a desktop icon: send it flying.
                if let Some((at, dir)) = ev.kicked {
                    if settings.move_icons && settings.desktop_icons {
                        let hit = icon_list.iter().position(|(_, r)| {
                            at.x > r.l - 10.0 * s && at.x < r.r + 10.0 * s && at.y > r.t - 10.0 * s && at.y < r.b + 10.0 * s
                        });
                        if let Some(index) = hit {
                            let (name, r) = &icon_list[index];
                            if !flights.iter().any(|f| f.index == index) {
                                flights.push(IconFlight {
                                    index,
                                    key: icon_key(name),
                                    pos: v2(r.l, r.t),
                                    vel: v2(dir * 700.0, -420.0) * s,
                                    size: v2(r.r - r.l, r.b - r.t),
                                    age: 0.0,
                                });
                            }
                        }
                    }
                }
                if let Some(target) = ev.poke {
                    pokes.push((me, target));
                }
                if let Some(target) = ev.high_five {
                    fives.push((me, target));
                }
                if ev.stomped {
                    match sweeper {
                        Some((who, look)) if who == me => {
                            guarded(&mut games_ok, || helper.stomped(&mut *plat, look, &mut rng));
                        }
                        _ => helper.cancel(),
                    }
                }
            }

            // What they did to each other.
            for (from, to, impulse) in hits.drain(..) {
                let victim_at = bodies.iter().find(|o| o.0 == to).map(|o| o.3);
                for b in buddies.iter_mut() {
                    if b.id == to {
                        b.man.knock(impulse);
                        b.brain.notify(BrainEvent::Hit { from });
                    } else if b.id != from {
                        // Bystanders see it, and may take sides.
                        if victim_at.is_some_and(|v| (v - b.man.center()).len() < 600.0 * s) {
                            b.brain.notify(BrainEvent::Saw { attacker: from, victim: to });
                        }
                    }
                }
            }
            for (from, to) in pokes {
                if let Some(b) = buddies.iter_mut().find(|b| b.id == to) {
                    if b.man.is_sleeping() {
                        b.man.interrupt();
                        b.brain.notify(BrainEvent::Poked { from });
                    }
                }
            }
            for (from, to) in fives {
                let mut answered = false;
                if let Some(b) = buddies.iter_mut().find(|b| b.id == to) {
                    if b.man.is_free() {
                        b.brain.notify(BrainEvent::HighFived { from });
                        b.man.interrupt();
                        answered = true;
                    }
                }
                if answered {
                    if let Some(b) = buddies.iter_mut().find(|b| b.id == from) {
                        b.brain.notify(BrainEvent::GaveHighFive { to });
                    }
                }
            }
        }
        world.peers = bodies.iter().map(|o| PeerBody { id: o.0, rect: o.1, ragdoll: o.2 }).collect();

        for b in buddies.iter_mut() {
            // You paying attention: hovering nearby...
            let c = b.man.center();
            b.attention = (b.attention - dt * 0.05).max(0.0);
            if (cursor - c).len() < 150.0 * s {
                b.attention = (b.attention + dt * 0.3).min(1.0);
                b.brain.notify(BrainEvent::Attention(dt * 0.15));
            }
            // ...or stroking it: the cursor moving gently over its head.
            let head = b.man.pose().head;
            let speed = cursor_vel.len();
            if !b.man.is_ragdoll() && (cursor - head).len() < 20.0 * s && speed > 60.0 * s && speed < 900.0 * s {
                b.pet += dt;
            } else {
                b.pet = (b.pet - dt * 0.5).max(0.0);
            }
            if b.pet > 0.8 {
                b.pet = 0.0;
                b.brain.notify(BrainEvent::Petted);
            }
            // How it feels comes out as body language.
            if let Some(f) = b.brain.take_feeling() {
                b.man.express(f);
            }
        }

        // Kicked icons flying across the desktop.
        let icon_g = stickman::GRAVITY * s * settings.gravity;
        let mut i = 0;
        while i < flights.len() {
            let moving = flights[i].step(dt, icon_g, &world);
            let (index, key, pos, size) = (flights[i].index, flights[i].key, flights[i].pos, flights[i].size);
            plat.move_icon(index, pos);
            let r = Rect { l: pos.x, t: pos.y, r: pos.x + size.x, b: pos.y + size.y };
            if let Some(entry) = world.icons.iter_mut().find(|(k, _)| *k == key) {
                entry.1 = r;
            }
            if let Some(entry) = icon_list.get_mut(index) {
                entry.1 = r;
            }
            if moving {
                i += 1;
            } else {
                flights.remove(i);
            }
        }

        for b in &buddies {
            let center = b.man.center();
            let origin = v2((center.x - size as f32 * 0.5).floor(), (center.y - size as f32 * 0.55).floor());
            if b.man.hidden() {
                pixmap.fill(tiny_skia::Color::TRANSPARENT);
            } else {
                render::draw(&mut pixmap, &b.man.pose(), origin, &d, &b.style, b.man.zzz());
            }
            let mut occluders = world.occluders(b.man.layer);
            // Diving into the taskbar: it covers them as they go in.
            if b.man.in_portal() {
                if let Some(tb) = world.taskbar {
                    occluders.push(tb.rect);
                }
            }
            // Up on a web page: it's *in* the page, so scrolling it out of
            // view hides it behind the browser's toolbar (and below the page).
            if b.man.in_page() {
                if let Some(page) = world.pages.iter().find(|p| Some(p.win) == b.man.room()) {
                    const FAR: f32 = 100_000.0;
                    occluders.push(Rect { l: -FAR, t: -FAR, r: FAR, b: page.doc.t });
                    occluders.push(Rect { l: -FAR, t: page.doc.b, r: FAR, b: FAR });
                }
            }
            plat.present(b.slot, pixmap.data(), size, size, origin.x as i32, origin.y as i32, &occluders);
        }

        // Saved now and then, in case of a crash or power cut.
        autosave -= dt;
        if autosave <= 0.0 {
            autosave = AUTOSAVE;
            for b in &buddies {
                roster::save(&b.saved(true));
            }
        }

        topmost -= dt;
        if topmost <= 0.0 {
            plat.keep_on_top();
            topmost = 2.0;
        }

        // ~60 fps while anyone's moving, ~30 fps while they all rest.
        let resting = buddies.iter().all(|b| b.man.is_resting());
        let frame = Duration::from_millis(if resting { 33 } else { 16 });
        let spent = frame_start.elapsed();
        if spent < frame {
            std::thread::sleep(frame - spent);
        }
    }

    // Closing: everyone out on the desktop is saved, and comes back next time.
    for b in &buddies {
        roster::save(&b.saved(true));
    }
}
