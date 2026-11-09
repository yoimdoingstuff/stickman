// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod brain;
mod games;
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
use games::minesweeper::{self, Helper, Look};
use games::{Game, GameEvent, Watcher};
use math::{v2, Rect, Vec2};
use peers::Peers;
use platform::{Event, MenuItem};
use render::Style;
use settings::Settings;
use stickman::{Dims, Mode, Stickman};
use world::{button_matches, WinId, World};

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
/// Gravity presets: menu ids MENU_GRAVITY + index.
const MENU_GRAVITY: u32 = 20;
const GRAVITY_PRESETS: [(&str, f32); 5] = [
    ("Moon (0.3x)", 0.3),
    ("Low (0.6x)", 0.6),
    ("Normal", 1.0),
    ("Heavy (1.6x)", 1.6),
    ("Jupiter (2.5x)", 2.5),
];
const MENU_QUIT: u32 = 99;

fn look_of(game: Game) -> Look {
    if game == Game::MinesweeperXp {
        Look::Xp
    } else {
        Look::Modern
    }
}

fn main() {
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
    let mut world =
        World { work: plat.work_area(), windows: plat.windows(), taskbar: plat.taskbar(), doors: Vec::new() };
    let mut peers = Peers::new();
    let mut watcher = Watcher::default();
    let mut helper = Helper::new(&mut rng);

    let spawn = v2(world.work.l + world.work.width() * rng.range(0.2, 0.8), world.work.t + 80.0 * s);
    let mut settings = Settings::load();
    let mut style = Style::new(settings.color, settings.outline);
    let mut man = Stickman::new(d, spawn);
    man.force_front = settings.stay_in_front;
    man.allow_cursor = settings.grab_cursor;
    man.gravity = settings.gravity;
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
    // The Minesweeper the helper is working on.
    let mut sweeper: Option<(WinId, Look)> = None;

    'run: loop {
        let frame_start = Instant::now();

        for ev in plat.poll_events() {
            match ev {
                Event::Quit => break 'run,
                Event::MouseDown => {
                    man.grab(plat.cursor());
                    brain.notify(BrainEvent::Grabbed);
                }
                Event::MouseUp => {
                    if man.release(&world) {
                        brain.notify(BrainEvent::Thrown);
                    }
                }
                Event::RightClick => {
                    let gravity_items = GRAVITY_PRESETS
                        .iter()
                        .enumerate()
                        .map(|(i, (label, g))| {
                            MenuItem::new(MENU_GRAVITY + i as u32, label, (settings.gravity - g).abs() < 0.01)
                        })
                        .collect();
                    let game_items = vec![
                        MenuItem::new(MENU_MS_HELP, "Help me with Minesweeper", settings.minesweeper_help),
                        MenuItem::new(MENU_MS_MISTAKES, "Let it make mistakes", settings.minesweeper_mistakes),
                        MenuItem::separator(),
                        MenuItem::new(MENU_MS_SNAPSHOT, "Save Minesweeper snapshot (for bug reports)", false),
                    ];
                    let items = [
                        MenuItem::new(MENU_LIMP, "Ragdoll", false),
                        MenuItem::new(MENU_COME, "Come here", false),
                        MenuItem::separator(),
                        MenuItem::new(MENU_FRONT, "Stay in front of windows", man.force_front),
                        MenuItem::new(MENU_CURSOR, "Can grab my cursor", man.allow_cursor),
                        MenuItem::new(MENU_PAUSE, "Pause thinking", paused),
                        MenuItem::submenu("Gravity", gravity_items),
                        MenuItem::submenu("Games", game_items),
                        MenuItem::separator(),
                        MenuItem::new(MENU_COLOR, "Change colour...", false),
                        MenuItem::new(MENU_OUTLINE, "Show outline", settings.outline),
                        MenuItem::separator(),
                        MenuItem::new(MENU_QUIT, "Quit", false),
                    ];
                    match plat.show_menu(&items) {
                        Some(MENU_LIMP) => man.go_limp(),
                        Some(MENU_COME) => {
                            let c = plat.cursor();
                            man.teleport(c - v2(0.0, 40.0 * s));
                        }
                        Some(MENU_FRONT) => {
                            man.force_front = !man.force_front;
                            settings.stay_in_front = man.force_front;
                            settings.save();
                        }
                        Some(MENU_CURSOR) => {
                            man.allow_cursor = !man.allow_cursor;
                            settings.grab_cursor = man.allow_cursor;
                            settings.save();
                        }
                        Some(MENU_PAUSE) => paused = !paused,
                        Some(MENU_COLOR) => {
                            if let Some(c) = plat.pick_color(settings.color) {
                                settings.color = c;
                                style = Style::new(c, settings.outline);
                                settings.save();
                            }
                        }
                        Some(MENU_OUTLINE) => {
                            settings.outline = !settings.outline;
                            style = Style::new(settings.color, settings.outline);
                            settings.save();
                        }
                        Some(MENU_MS_HELP) => {
                            settings.minesweeper_help = !settings.minesweeper_help;
                            settings.save();
                        }
                        Some(MENU_MS_MISTAKES) => {
                            settings.minesweeper_mistakes = !settings.minesweeper_mistakes;
                            settings.save();
                        }
                        Some(MENU_MS_SNAPSHOT) => {
                            if let Some((id, game)) = watcher.find(Game::is_minesweeper) {
                                let result = minesweeper::snapshot(&mut *plat, id, look_of(game));
                                // Show the folder with the picture in it.
                                let path = match result {
                                    Ok(p) => Some(p),
                                    Err(e) => {
                                        eprintln!("stickman: {e}");
                                        None
                                    }
                                };
                                if let Some(dir) = std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join("stickman")) {
                                    let target = path.unwrap_or(dir);
                                    let _ = std::process::Command::new("explorer").arg("/select,").arg(target).spawn();
                                }
                            }
                        }
                        Some(MENU_QUIT) => break 'run,
                        Some(id) if (MENU_GRAVITY..MENU_GRAVITY + GRAVITY_PRESETS.len() as u32).contains(&id) => {
                            settings.gravity = GRAVITY_PRESETS[(id - MENU_GRAVITY) as usize].1;
                            man.gravity = settings.gravity;
                            settings.save();
                        }
                        _ => {}
                    }
                    // The menu blocks; don't simulate the time it was open.
                    last = Instant::now();
                }
            }
        }

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32().min(0.1);
        last = now;

        // Remember where every window was, so moves are never missed (even on
        // the frames where the whole list is re-scanned).
        let before: Vec<(WinId, Rect)> = world.windows.iter().map(|w| (w.id, w.rect)).collect();
        scan -= dt;
        if scan <= 0.0 {
            world.windows = plat.windows();
            world.work = plat.work_area();
            world.taskbar = plat.taskbar();
            scan = WINDOW_SCAN;
            // Games opening, being won or lost.
            for e in watcher.update(&*plat, &world) {
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

        // Other stickmen: share our state, get knocked over by theirs.
        for hit in peers.update(dt, man.center(), man.vel, man.is_ragdoll()) {
            man.knock(hit.impulse);
            brain.notify(BrainEvent::Hit { from: hit.from });
        }

        // Thrown or flying fast into another stickman bowls it over.
        body_hit_cooldown -= dt;
        let flying = man.is_ragdoll() || man.mode == Mode::Air;
        if flying && body_hit_cooldown <= 0.0 && man.vel.len() > 900.0 * s {
            let me = man.center();
            for peer in peers.list.iter().filter(|p| !p.ragdoll && (p.hip - me).len() < 30.0 * s) {
                peers.send_hit(peer.id, man.vel * 0.5);
                body_hit_cooldown = 0.6;
            }
        }

        // Helping with Minesweeper while you play it.
        sweeper = None;
        if settings.minesweeper_help && !paused {
            if let Some((id, game)) = watcher.find(Game::is_minesweeper) {
                if foreground == Some(id) {
                    let look = look_of(game);
                    sweeper = Some((id, look));
                    let ready = man.can_help();
                    let mistakes = settings.minesweeper_mistakes;
                    if let Some(at) = helper.update(dt, &mut *plat, id, look, ready, mistakes, &mut rng) {
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
                let views =
                    peers.list.iter().map(|p| PeerView { id: p.id, hip: p.hip, ragdoll: p.ragdoll }).collect();
                if let Some(p) = man.perceive(cursor, views, focus_game) {
                    let intent = brain.decide(&p, &world, &mut rng);
                    man.apply(intent, &world);
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
            if ev.stomped {
                match sweeper {
                    Some((_, look)) => helper.stomped(&mut *plat, look, &mut rng),
                    None => helper.cancel(),
                }
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
