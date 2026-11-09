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
- **Right-click** it for a short menu: Ragdoll, Come here, Settings..., Quit.
  **Settings** opens a panel with everything else: colour, outline, stay in
  front, cursor grabbing, desktop icons, web pages, pause, gravity, and the
  game helpers.
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
back out over the top. It only dives into the target window's own taskbar
button, found with UI Automation, so it never jumps into the Start button
or tray icons. This works with the stock Windows 11 taskbar and
ExplorerPatcher's, on any edge (top-docked taskbars just skip the diving;
auto-hide is detected). Windows are solid while walking too, and ragdolls
bounce off them.

**Walking.** A proper gait cycle: each foot is planted (never sliding) for
part of the cycle and swings forward in an arc the rest of the time, lifting
the knee early and reaching out to land ahead. Walking, the hips vault over
the planted leg (highest mid-step, lowest with both feet down); running,
they compress on each landing and float between steps, and the arms swing
opposite the legs.

**Walk styles.** Normal, sneaking (tiptoe, hands up), strutting, marching,
skipping, and a tired shuffle, picked from its mood.

**Health (hidden).** Hard landings, slams, hits from other stickmen and
being squashed all hurt. Below half health it limps; badly hurt it can only
crawl on hands and knees and won't do anything acrobatic. It heals slowly
over time, faster while asleep.

**Squashing.** Drag a window into it and it gets shoved along, or flattened
like a pancake against the screen edge (or the floor, if a window comes
down on it), then pops out limp.

**Unpredictable.** Every so often a random impulse takes over: it trips
over nothing, throws a tantrum on the floor, plays dead (and jumps up when
your cursor comes near), moonwalks, runs screaming from your cursor, spins,
does a baseball slide, leaps sky-high, or runs off one side of the screen
and comes back in the other. Its personality also drifts over time, and now
and then it ignores all its reasoning and just does something.

**Stacking.** Stickmen can jump onto each other's heads and get carried
around, and ragdolls pile up on each other. A stickman may sneak over to
one that's asleep and poke it awake (or, if it's mischievous, kick it).

**Fights.** Once two stickmen start fighting they keep at it for several
seconds: closing in, mixing punches and kicks, backing off, guarding, and
waiting for the other to get up.

**Moving windows.** It isn't glued on: friction only lets it keep up so
fast, so if you yank a window it slides out from under it (it stumbles,
windmills its arms, slips off the edge or goes over), and if you stop
suddenly it skids on. Drag it down faster than it can fall and it gets left
behind. Minimizing the window it's on, or a big fall, can send it flailing.
Maximized windows count as background (nothing to climb or stand on).

**Classic games.** It recognises the XP, Vista (including the Ultimate
Extras, Texas Hold 'Em and Tinker) and Windows 7 games, including the
win7games.com versions, by their program names. It wanders over to watch
you play, dances when you win and sulks when you lose.

**Minesweeper helper.** Every few tiles you uncover, it leaps onto a tile and
stomps it open for you. It only picks tiles the visible numbers *prove* are
safe, ignores your flags (a wrong one could trick it), and re-checks the
board right before pressing, so it can't lose the game for you. Turn on
"Let it make mistakes" (Games menu) if you'd like it to occasionally gamble.
It reads the board from the game's accessibility tree (Windows 7
Minesweeper lists every tile, e.g. "Row 3, Column 5 Tile (Concealed)"), and
presses tiles through it too, so your mouse isn't touched. If that's not
available it falls back to reading the screen. If it isn't helping, use
"Save Minesweeper snapshot" in the Games menu: it saves `minesweeper.png`
and `minesweeper.txt` (what it sees and why it isn't helping) in
`%APPDATA%\stickman`.

**Desktop icons.** Your desktop icons are ledges too: it jumps onto them,
walks across them, sits on their edges, and rides along if you drag one.
It also leaps up, grabs the underside of an icon and swings on it like a
gymnastics bar, pumping its legs to go higher before launching off (often
with a flip). Turn it off in Settings if you'd rather it didn't.

**Web pages.** In Chrome, Edge, Firefox, Brave, Opera, Vivaldi and other
browsers, the boxes on the page you're looking at (text blocks, images,
buttons, links) become ledges. Once it's inside the browser window (it
likes to dive in through the taskbar to explore, or drop it in yourself) it
hops from box to box, walks along them, sits on their edges and falls off
them. It reacts to scrolling straight away (it watches the mouse wheel, and
nothing else, over pages): scroll down and it's carried up and off the top,
into the page, hidden behind the browser's toolbar until you scroll back;
scroll up fast and the page drops away under it, so it falls down through
the page onto whatever's below. Other apps work the same way once it's
inside them: Discord, Spotify, Slack and friends (built on a browser
engine), and anything else that describes itself to accessibility tools,
including cards and tiles in some games.

**Kicking icons** (off by default, since it really moves them): with "Can
kick my desktop icons around" on, it kicks icons it's next to and they fly
across the screen and bounce to a stop. If Explorer's "Auto arrange icons"
is on, they snap back.

**Multiple screens.** It roams all your monitors, walking along the bottom
of each one.

**Learning what you like** (off by default): with "Learn what I like" on,
it logs each thing it does and whether you reacted (picked it up, hovered
over it) to `%APPDATA%\stickman\behaviour_log.jsonl`. Run
`python training/train.py` (needs `pip install numpy`) to train a small
model from that log; next time it starts, it leans towards the things you
react to. Nothing leaves your PC.

**Gravity** can be changed from the right-click menu (Moon to Jupiter).
Lower gravity also means higher, floatier jumps.

When it's standing around it fidgets: looks around (and at your cursor),
stretches, scratches its head, or crosses its arms and taps its foot when
bored. When tired it naps (with floating Z's) or meditates; with energy to
spare it does push-ups, shadowboxes or practises kicks.

If it ever crashes, the reason is written to `%APPDATA%\stickman\crash.txt`
(newest first). Problems in the game helpers just switch those off for the
rest of the session instead of taking the stickman down.

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
| `src/learn.rs`, `training/train.py` | Logging what it does and how you react; training and loading the "what you like" model. |
| `src/games/` | Recognising the classic games, and the Minesweeper helper. |
| `src/render.rs` | Draws the pose with tiny-skia. |
| `src/brain.rs` | `Brain` trait + a utility AI with personality and needs (boredom, energy). Meant to be swapped for a small trained model later. |

**Walking behind windows:** the overlay is always on top, but each frame the
pixels covered by windows that are "in front" of the stickman's layer are cut
out, so it looks like it's behind them.

## Roadmap

1. ~~Procedural stickman, walking, jumping, climbing, behind-window occlusion~~
2. ~~Stand on things inside other apps too (Discord, media players)~~
3. ~~Stand on cards / tiles in the other classic games~~ (where the game
   exposes them to accessibility tools)
4. ~~Kick desktop icons around (opt-in)~~; picking them up and carrying them
5. ~~Multi-monitor support~~
6. ~~Learn from you: log, Python trainer, model loaded in Rust~~
7. macOS / Linux backends (everything OS-specific is behind the `Platform`
   trait in `src/platform/`, so a backend is "only" that file's worth of
   window, overlay and accessibility code per OS)
