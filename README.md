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

- **Left-drag** the stickman to pick it up and throw it.
- **Right-click** it for the menu: stay in front, pause, come here,
  change colour, quit.

Settings are saved to `%APPDATA%\stickman\settings.txt`:

```
color=#000000
stay_in_front=false
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
| `src/render.rs` | Draws the pose with tiny-skia. |
| `src/brain.rs` | `Brain` trait + a utility AI with personality and needs (boredom, energy). Meant to be swapped for a small trained model later. |

**Walking behind windows:** the overlay is always on top, but each frame the
pixels covered by windows that are "in front" of the stickman's layer are cut
out, so it looks like it's behind them.

## Roadmap

1. ~~Procedural stickman, walking, jumping, climbing, behind-window occlusion~~
2. Settings/permissions panel + config file
3. Multi-monitor support
4. Train a small behavior model in Python, export weights, run inference in Rust
5. macOS / Linux backends
