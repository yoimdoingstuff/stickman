// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod brain;
mod games;
mod learn;
mod math;
mod peers;
mod platform;
mod ragdoll;
mod render;
mod rig;
mod rng;
mod settings;
mod stickman;
mod world;

use std::time::{Duration, Instant};

use brain::{Brain, BrainEvent, PeerView, Personality, UtilityBrain};
use games::minesweeper::{Helper, Look};
use games::{Game, GameEvent, Watcher};
use math::{v2, Rect, Vec2};
use peers::{Incoming, Me, Peers};
use platform::{Event, MenuItem, SettingItem};
use render::Style;
use settings::Settings;
use stickman::{Dims, Mode, Stickman};
use world::{button_matches, icon_key, page_ledges, Page, PeerBody, WinId, World};

/// Fixed physics/animation step.
const STEP: f32 = 1.0 / 120.0;
/// How often to re-scan all windows (the one we're on is polled every frame).
const WINDOW_SCAN: f32 = 0.15;
/// How often to re-read the taskbar's buttons.
const DOOR_SCAN: f32 = 2.0;
/// Base size multiplier on top of the monitor's DPI scale.
const SIZE: f32 = 1.15;

const MENU_FRONT: u32 = 1;
const MENU_PAUSE: u32 = 2;
const MENU_COME: u32 = 3;
const MENU_COLOR: u32 = 4;
const MENU_LIMP: u32 = 5;
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

/// How far one mouse-wheel notch scrolls a page (at 100% scaling).
fn scroll_step(exe: &str) -> f32 {
    match exe {
        "firefox.exe" | "librewolf.exe" | "waterfox.exe" | "floorp.exe" | "zen.exe" => 57.0,
        _ => 100.0,
    }
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
fn settings_items(settings: &Settings, man: &Stickman, paused: bool) -> Vec<SettingItem> {
    let check = |id: u32, label: &str, on: bool| SettingItem::Check { id, label: label.to_string(), on };
    let button = |id: u32, label: &str| SettingItem::Button { id, label: label.to_string() };
    let heading = |t: &str| SettingItem::Heading(t.to_string());
    let gravity = GRAVITY_PRESETS.iter().position(|(_, g)| (settings.gravity - g).abs() < 0.01).unwrap_or(2);
    vec![
        heading("Looks"),
        button(MENU_COLOR, "Change colour..."),
        check(MENU_OUTLINE, "Show outline", settings.outline),
        heading("Behaviour"),
        check(MENU_FRONT, "Always stay in front of windows", man.force_front),
        check(MENU_CURSOR, "Can grab my mouse cursor", man.allow_cursor),
        check(MENU_ICONS, "Play on my desktop icons", settings.desktop_icons),
        check(MENU_WEB, "Explore web pages and apps", settings.web_pages),
        check(MENU_MOVE_ICONS, "Can kick my desktop icons around (really moves them)", settings.move_icons),
        check(MENU_LEARN, "Learn what I like (keeps a log on this PC)", settings.learn),
        check(MENU_PAUSE, "Pause thinking", paused),
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

fn main() {
    install_crash_log();
    let mut plat = match platform::create() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("stickman: {e}");
            return;
        }
    };
    let d = Dims::new(plat.dpi_scale() * SIZE);
    let s = d.s;
    let mut rng = rng::Rng::seeded();
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
    // Each window's program (looked up once per window).
    let mut exes: std::collections::HashMap<WinId, String> = std::collections::HashMap::new();
    let mut page_scan = 0.0f32;
    // Page scrolling predicted from the mouse wheel, still to be applied
    // (px, + = page content moving down), and when the wheel last turned.
    let mut scroll: std::collections::HashMap<WinId, f32> = std::collections::HashMap::new();
    // Desktop icons in their real order (index = what Explorer calls them),
    // and icons currently flying after being kicked.
    let mut icon_list: Vec<(String, Rect)> = Vec::new();
    let mut flights: Vec<IconFlight> = Vec::new();
    // What it did and whether you reacted (only kept with "Learn what I like").
    let mut logger = learn::Logger::default();
    let mut last_wheel: std::collections::HashMap<WinId, Instant> = std::collections::HashMap::new();
    let mut icon_scan = 0.0f32;
    let mut peers = Peers::new();
    let mut watcher = Watcher::default();
    let mut helper = Helper::new(&mut rng);
    // Turned off for the rest of the session if the game code ever panics.
    let mut games_ok = true;

    let spawn = v2(world.work.l + world.work.width() * rng.range(0.2, 0.8), world.work.t + 80.0 * s);
    let mut settings = Settings::load();
    let mut style = Style::new(settings.color, settings.outline);
    let mut man = Stickman::new(d, spawn);
    man.force_front = settings.stay_in_front;
    man.allow_cursor = settings.grab_cursor;
    man.gravity = settings.gravity;
    man.move_icons = settings.move_icons;
    let mut brain = UtilityBrain::new(Personality::random(&mut rng));

    let size = ((190.0 * s).ceil() as u32).max(64);
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("pixmap");

    let mut paused = false;
    let mut last = Instant::now();
    let mut acc = 0.0f32;
    let mut scan = 0.0f32;
    let mut door_scan = 0.0f32;
    let mut topmost = 0.0f32;
    let mut last_cursor = plat.cursor();
    let mut body_hit_cooldown = 0.0f32;

    'run: loop {
        let frame_start = Instant::now();

        for ev in plat.poll_events() {
            // A command from the right-click menu (value -1 = toggle) or the
            // settings panel (value = new checkbox state / dropdown index).
            let (id, value) = match ev {
                Event::Quit => break 'run,
                Event::MouseDown => {
                    man.grab(plat.cursor());
                    brain.notify(BrainEvent::Grabbed);
                    logger.engaged(1.0);
                    continue;
                }
                Event::MouseUp => {
                    if man.release(&world) {
                        brain.notify(BrainEvent::Thrown);
                    }
                    continue;
                }
                Event::RightClick => {
                    // Kept short: everything else is in the settings panel.
                    let items = [
                        MenuItem::new(MENU_LIMP, "Ragdoll", false),
                        MenuItem::new(MENU_COME, "Come here", false),
                        MenuItem::separator(),
                        MenuItem::new(MENU_SETTINGS, "Settings...", false),
                        MenuItem::separator(),
                        MenuItem::new(MENU_QUIT, "Quit", false),
                    ];
                    let picked = plat.show_menu(&items);
                    // The menu blocks; don't simulate the time it was open.
                    last = Instant::now();
                    match picked {
                        Some(id) => (id, -1),
                        None => continue,
                    }
                }
                Event::Setting { id, value } => (id, value),
                Event::Wheel { at, notches } => {
                    // Scrolling a page it's on (or near): move the page's
                    // boxes straight away instead of waiting for a re-read.
                    if let Some(id) = world.window_at(at) {
                        if world.pages.iter().any(|p| p.win == id) {
                            let exe = exes.get(&id).map(String::as_str).unwrap_or("");
                            *scroll.entry(id).or_insert(0.0) += notches * scroll_step(exe) * plat.dpi_scale();
                            last_wheel.insert(id, Instant::now());
                        }
                    }
                    continue;
                }
            };
            let flag = |current: bool| if value < 0 { !current } else { value != 0 };
            match id {
                MENU_LIMP => man.go_limp(),
                MENU_COME => {
                    let c = plat.cursor();
                    man.teleport(c - v2(0.0, 40.0 * s));
                }
                MENU_SETTINGS => plat.open_settings(&settings_items(&settings, &man, paused)),
                MENU_FRONT => {
                    man.force_front = flag(man.force_front);
                    settings.stay_in_front = man.force_front;
                    settings.save();
                }
                MENU_CURSOR => {
                    man.allow_cursor = flag(man.allow_cursor);
                    settings.grab_cursor = man.allow_cursor;
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
                    man.move_icons = settings.move_icons;
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
                        style = Style::new(c, settings.outline);
                        settings.save();
                    }
                    last = Instant::now();
                }
                MENU_OUTLINE => {
                    settings.outline = flag(settings.outline);
                    style = Style::new(settings.color, settings.outline);
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
                        man.gravity = settings.gravity;
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
                if matches!(e, GameEvent::Finished(..)) {
                    man.interrupt();
                }
                brain.notify(BrainEvent::Game(e));
            }
        } else {
            if let Some(id) = man.attached_window() {
                world.set_window_rect(id, plat.window_rect(id));
            }
            // The foreground window is the one being dragged around: track it
            // every frame.
            if let Some(id) = world.windows.first().map(|w| w.id) {
                world.set_window_rect(id, plat.window_rect(id));
            }
        }
        // Any window that moved can shove or squash the stickman.
        for (id, old) in before {
            if let Some(new) = world.window(id).map(|w| w.rect) {
                if new != old && Some(id) != man.attached_window() {
                    man.shove(&world, id, old, new, dt);
                }
            }
        }

        // Which taskbar button leads to which window (it only ever dives
        // into a real app button, for the window it's heading to).
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

        // Pages: the boxes on the web page (or in the app) it's inside, and in
        // the browser you're using, become ledges it can stand on.
        page_scan -= dt;
        if !settings.web_pages {
            world.pages.clear();
            scroll.clear();
        } else if page_scan <= 0.0 {
            page_scan = 0.5;
            exes.retain(|id, _| world.window(*id).is_some());
            for w in &world.windows {
                exes.entry(w.id).or_insert_with(|| plat.window_process(w.id).map(|p| p.1).unwrap_or_default());
            }
            let mut wanted: Vec<WinId> = Vec::new();
            // Whatever window it's inside (any app), plus the browser in front.
            if let Some(id) = man.room() {
                wanted.push(id);
            }
            if let Some(id) = foreground {
                if exes.get(&id).is_some_and(|e| has_web_content(e)) && !wanted.contains(&id) {
                    wanted.push(id);
                }
            }
            let mut pages = Vec::new();
            for id in wanted {
                let current = world.pages.iter().find(|p| p.win == id).cloned();
                let fresh = plat.page_boxes(id).filter(|(_, _, age)| *age < 10.0);
                let page = match fresh {
                    Some((doc, boxes, age)) => {
                        // Read before the scrolling settled? Then it's older
                        // than what we've predicted: keep the prediction.
                        let taken = Instant::now().checked_sub(Duration::from_secs_f32(age));
                        let settled = last_wheel.get(&id).map(|w| *w + Duration::from_millis(400));
                        let outdated = matches!((taken, settled), (Some(t), Some(w)) if t < w);
                        if outdated && current.is_some() {
                            current
                        } else {
                            scroll.remove(&id);
                            Some(Page { win: id, doc, ledges: page_ledges(doc, &boxes, s) })
                        }
                    }
                    None => current,
                };
                pages.extend(page);
            }
            world.pages = pages;
        }
        // Play out predicted scrolling smoothly (like the browser does).
        for (id, pending) in scroll.iter_mut() {
            if pending.abs() < 0.1 {
                continue;
            }
            let step = *pending * (1.0 - (-dt * 14.0).exp());
            *pending -= step;
            if let Some(page) = world.pages.iter_mut().find(|p| p.win == *id) {
                for r in &mut page.ledges {
                    r.t += step;
                    r.b += step;
                }
            }
        }

        // Other stickmen: share our state, get knocked over (or woken) by theirs.
        let me = Me {
            hip: man.center(),
            vel: man.vel,
            ragdoll: man.is_ragdoll(),
            sleeping: man.is_sleeping(),
            body: man.body_rect(),
        };
        for msg in peers.update(dt, &me) {
            match msg {
                Incoming::Hit { from, impulse } => {
                    man.knock(impulse);
                    brain.notify(BrainEvent::Hit { from });
                }
                Incoming::Poke { from } => {
                    if man.is_sleeping() {
                        man.interrupt();
                        brain.notify(BrainEvent::Poked { from });
                    }
                }
            }
        }
        // Their bodies, for standing on and piling up on.
        world.peers = peers.list.iter().map(|p| PeerBody { id: p.id, rect: p.body, ragdoll: p.ragdoll }).collect();
        // A random impulse takes over whatever it's doing (but lets it sleep).
        if brain.whim_due() && !man.is_sleeping() {
            man.interrupt();
        }

        // Thrown or flying fast into another stickman bowls it over (and
        // the hit slows us down).
        body_hit_cooldown -= dt;
        let flying = man.is_ragdoll() || man.mode == Mode::Air;
        if flying && body_hit_cooldown <= 0.0 && man.vel.len() > 600.0 * s {
            let mine = man.body_rect();
            let hit: Vec<u32> = peers.list.iter().filter(|p| !p.ragdoll && p.body.overlaps(&mine)).map(|p| p.id).collect();
            if !hit.is_empty() {
                for id in hit {
                    peers.send_hit(id, man.vel * 0.55);
                }
                man.bounce(man.vel * -0.35);
                body_hit_cooldown = 0.5;
            }
        }

        // Helping with Minesweeper while you play it (the game it's working on).
        let mut sweeper: Option<(WinId, Look)> = None;
        if settings.minesweeper_help && !paused && games_ok {
            if let Some((id, game)) = watcher.find(Game::is_minesweeper) {
                if foreground == Some(id) {
                    let look = look_of(game);
                    sweeper = Some((id, look));
                    let ready = man.can_help();
                    let mistakes = settings.minesweeper_mistakes;
                    let target =
                        guarded(&mut games_ok, || helper.update(dt, &mut *plat, id, look, ready, mistakes, &mut rng)).flatten();
                    if let Some(at) = target {
                        if !man.stomp(at, id) {
                            helper.cancel();
                        }
                    }
                }
            }
        }

        acc = (acc + dt).min(0.25);
        while acc >= STEP {
            acc -= STEP;
            if !paused && man.wants_intent() {
                let views = peers
                    .list
                    .iter()
                    .map(|p| PeerView { id: p.id, hip: p.hip, ragdoll: p.ragdoll, sleeping: p.sleeping, ground: p.body.b })
                    .collect();
                if let Some(p) = man.perceive(cursor, views, focus_game) {
                    let intent = brain.decide(&p, &world, &mut rng);
                    man.apply(intent, &world);
                    if let Some((kind, x)) = brain.take_choice() {
                        if settings.learn {
                            logger.chose(kind, x);
                        }
                    }
                }
            }
            brain.tick(STEP);
            let ev = man.update(STEP, &world, cursor, cursor_vel);
            if let Some(impact) = ev.landed {
                brain.notify(BrainEvent::Landed(impact));
            }
            if let Some((target, impulse, range)) = ev.hit {
                if let Some(peer) = peers.get(target) {
                    if (peer.hip - man.pos).len() < range {
                        peers.send_hit(target, impulse);
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
                peers.send_poke(target);
            }
            if ev.stomped {
                match sweeper {
                    Some((_, look)) => {
                        guarded(&mut games_ok, || helper.stomped(&mut *plat, look, &mut rng));
                    }
                    None => helper.cancel(),
                }
            }
        }

        // Learning: hovering your mouse near it counts as paying attention.
        if settings.learn {
            if (cursor - man.center()).len() < 150.0 * s {
                logger.engaged(dt * 0.6);
            }
            logger.tick();
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

        let center = man.center();
        let origin = v2((center.x - size as f32 * 0.5).floor(), (center.y - size as f32 * 0.55).floor());
        if man.hidden() {
            pixmap.fill(tiny_skia::Color::TRANSPARENT);
        } else {
            render::draw(&mut pixmap, &man.pose(), origin, &d, &style, man.zzz());
        }
        let mut occluders = world.occluders(man.layer);
        // Diving into the taskbar: it covers us as we go in.
        if man.in_portal() {
            if let Some(tb) = world.taskbar {
                occluders.push(tb.rect);
            }
        }
        // Up on a web page: it's *in* the page, so scrolling it out of view
        // hides it behind the browser's toolbar (and below the page).
        if man.in_page() {
            if let Some(page) = world.pages.iter().find(|p| Some(p.win) == man.room()) {
                const FAR: f32 = 100_000.0;
                occluders.push(Rect { l: -FAR, t: -FAR, r: FAR, b: page.doc.t });
                occluders.push(Rect { l: -FAR, t: page.doc.b, r: FAR, b: FAR });
            }
        }
        plat.present(pixmap.data(), size, size, origin.x as i32, origin.y as i32, &occluders);

        topmost -= dt;
        if topmost <= 0.0 {
            plat.keep_on_top();
            topmost = 2.0;
        }

        // ~60 fps while moving, ~30 fps while resting to save CPU.
        let frame = Duration::from_millis(if man.is_resting() { 33 } else { 16 });
        let spent = frame_start.elapsed();
        if spent < frame {
            std::thread::sleep(frame - spent);
        }
    }
}
