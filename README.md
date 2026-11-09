# stickman

A procedurally animated stickman that lives on your desktop, inspired by
*Animator vs. Animation*. It walks on the taskbar, jumps onto windows, climbs
their sides, walks behind them, and can be picked up and thrown. Everything
runs offline.

## Download

Every push to `main` is built by GitHub Actions (`.github/workflows/build.yml`).
Open the **Actions** tab, pick the latest **Build** run, and download the
`stickman-windows` artifact (it contains `stickman.exe`). Pushing a tag like
`v0.1.0` also attaches the exe to a GitHub Release.

To build locally instead: install Rust (https://rustup.rs) and run
`cargo run --release`. Windows only for now.

## Use

- **Left-drag** any part of the stickman to pick it up by that limb (it goes
  ragdoll). Throw it hard and it flops; drop it gently and it lands on its feet.
- **Drop it onto a window** and it lives inside that window, walking along
  its bottom edge until it decides to jump out.
- **Right-click** it for the menu: ragdoll, come here, stay in front, allow
  cursor grabbing, pause, change colour, outline on/off, quit.
- **Run `stickman.exe` more than once** (up to 16) and the stickmen notice
  each other: they wander over, wave, punch, hold grudges, and knock each
  other over when thrown into one another. They talk over UDP on
  `127.0.0.1` only, so nothing leaves your PC.

On its own it walks, runs, sits, dances, waves, backflips, cartwheels, jumps
between windows, climbs window sides, walks behind windows, hops into
windows, sits on window ledges with its legs dangling, and jumps up to hang
off your mouse cursor (shake the mouse to fling it off).

**Getting around.** Windows are solid while it's in the air: it bonks its
head on window bottoms and bounces off their sides, and it only jumps when
the arc is clear. Otherwise it goes around: climbs a window's outer wall,
drops off edges, or dives into the taskbar and pops out on top of (or
inside) another window. Inside a window it climbs the inner wall to get
back out over the top. The taskbar is found with the standard appbar API,
so this works with the stock Windows 11 taskbar and ExplorerPatcher's, on
any edge (top-docked taskbars just skip the diving; auto-hide is detected).

**Walk styles.** Normal, sneaking (tiptoe, hands up), strutting, marching,
skipping, and a tired shuffle, picked from its mood.

**Health (hidden).** Hard landings, slams, hits from other stickmen and
being squashed all hurt. Below half health it limps; badly hurt it can only
crawl on hands and knees and won't do anything acrobatic. It heals slowly
over time, faster while asleep.

**Squashing.** Drag a window into it and it gets shoved along, or flattened
like a pancake against the screen edge (or the floor, if a window comes
down on it), then pops out limp.

**Fights.** Once two stickmen start fighting they keep at it for several
seconds: closing in, mixing punches and kicks, backing off, guarding, and
waiting for the other to get up.

**Gravity** can be changed from the right-click menu (Moon to Jupiter).
Lower gravity also means higher, floatier jumps.

When it's standing around it fidgets: looks around (and at your cursor),
stretches, scratches its head, or crosses its arms and taps its foot when
bored. When tired it naps (with floating Z's) or meditates; with energy to
spare it does push-ups, shadowboxes or practises kicks.

Settings are saved to `%APPDATA%\stickman\settings.txt`:

```
color=#000000
outline=true
stay_in_front=false
grab_cursor=true
gravity=1
```

Any colour works. Dark colours get a light outline and light colours a dark
one, so it stays visible on any background.

## How it works

| File | What it does |
| --- | --- |
| `src/platform/` | OS layer behind a `Platform` trait. `win32.rs` is a tiny per-pixel-alpha layered window that follows the stickman, plus window enumeration in z-order. |
| `src/world.rs` | Turns windows into surfaces (top edges), walls (side edges) and occluders (windows above the stickman's layer). |
| `src/stickman.rs` | Body physics and actions: walk, jump (ballistic, aimed at a landing spot), climb, mantle, drag/throw. |
| `src/rig.rs` | Procedural animation: planted feet/hands that step when they drift too far, two-bone IK for knees/elbows, springs for lean, squash and dangling limbs. No keyframes. |
| `src/ragdoll.rs` | Verlet ragdoll: joints are particles, bones are distance constraints, with pins for dragging and cursor-hanging. |
| `src/peers.rs` | Finds other running stickmen over localhost UDP and sends/receives hits. |
| `src/settings.rs` | Loads/saves the settings file. |
| `src/render.rs` | Draws the pose with tiny-skia. |
| `src/brain.rs` | `Brain` trait + a utility AI with personality and needs (boredom, energy). Meant to be swapped for a small trained model later. |

**Walking behind windows:** the overlay is always on top, but each frame the
pixels covered by windows that are "in front" of the stickman's layer are cut
out, so it looks like it's behind them.

## Roadmap

1. ~~Procedural stickman, walking, jumping, climbing, behind-window occlusion~~
2. Stand on things *inside* windows (message boxes, images, album covers)
   using Windows UI Automation to find their rectangles
3. Dive into the actual taskbar button of the target window (also via UI
   Automation) instead of wherever it's standing
4. Pick up and carry desktop icons (behind a permission toggle)
5. Settings/permissions panel
6. Multi-monitor support
7. Train a small behavior model in Python, export weights, run inference in Rust
8. macOS / Linux backends
