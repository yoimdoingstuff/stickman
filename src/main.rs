// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod brain;
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
use math::{v2, Vec2};
use peers::Peers;
use platform::{Event, MenuItem};
use render::Style;
use settings::Settings;
use stickman::{Dims, Mode, Stickman};
use world::World;

/// Fixed physics/animation step.
const STEP: f32 = 1.0 / 120.0;
/// How often to re-scan all windows (the one we're on is polled every frame).
const WINDOW_SCAN: f32 = 0.15;
/// Base size multiplier on top of the monitor's DPI scale.
const SIZE: f32 = 1.15;

const MENU_FRONT: u32 = 1;
const MENU_PAUSE: u32 = 2;
const MENU_COME: u32 = 3;
const MENU_COLOR: u32 = 4;
const MENU_LIMP: u32 = 5;
const MENU_OUTLINE: u32 = 6;
const MENU_CURSOR: u32 = 7;
const MENU_QUIT: u32 = 99;

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
    let mut world = World { work: plat.work_area(), windows: plat.windows() };
    let mut peers = Peers::new();

    let spawn = v2(world.work.l + world.work.width() * rng.range(0.2, 0.8), world.work.t + 80.0 * s);
    let mut settings = Settings::load();
    let mut style = Style::new(settings.color, settings.outline);
    let mut man = Stickman::new(d, spawn);
    man.force_front = settings.stay_in_front;
    man.allow_cursor = settings.grab_cursor;
    let mut brain = UtilityBrain::new(Personality::random(&mut rng));

    let size = ((190.0 * s).ceil() as u32).max(64);
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("pixmap");

    let mut paused = false;
    let mut last = Instant::now();
    let mut acc = 0.0f32;
    let mut scan = 0.0f32;
    let mut topmost = 0.0f32;
    let mut last_cursor = plat.cursor();
    let mut body_hit_cooldown = 0.0f32;

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
                    let items = [
                        MenuItem::new(MENU_LIMP, "Ragdoll", false),
                        MenuItem::new(MENU_COME, "Come here", false),
                        MenuItem::separator(),
                        MenuItem::new(MENU_FRONT, "Stay in front of windows", man.force_front),
                        MenuItem::new(MENU_CURSOR, "Can grab my cursor", man.allow_cursor),
                        MenuItem::new(MENU_PAUSE, "Pause thinking", paused),
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
                        Some(MENU_QUIT) => break 'run,
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

        scan -= dt;
        if scan <= 0.0 {
            world.windows = plat.windows();
            world.work = plat.work_area();
            scan = WINDOW_SCAN;
        } else if let Some(id) = man.attached_window() {
            world.set_window_rect(id, plat.window_rect(id));
        }

        let cursor = plat.cursor();
        let cursor_vel = if dt > 0.0 { (cursor - last_cursor) / dt } else { Vec2::ZERO };
        last_cursor = cursor;

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

        acc = (acc + dt).min(0.25);
        while acc >= STEP {
            acc -= STEP;
            if !paused && man.wants_intent() {
                let views =
                    peers.list.iter().map(|p| PeerView { id: p.id, hip: p.hip, ragdoll: p.ragdoll }).collect();
                if let Some(p) = man.perceive(cursor, views) {
                    let intent = brain.decide(&p, &world, &mut rng);
                    man.apply(intent);
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
        }

        let center = man.center();
        let origin = v2((center.x - size as f32 * 0.5).floor(), (center.y - size as f32 * 0.55).floor());
        render::draw(&mut pixmap, &man.pose(), origin, &d, &style, man.zzz());
        let occluders = world.occluders(man.layer);
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
