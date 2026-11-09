//! The classic Windows games: XP, Vista (including the Ultimate Extras) and
//! 7, plus the win7games.com re-releases, which use the same programs.
//! Games are recognised by their program name, so it doesn't matter what
//! language the titles are in.

pub mod minesweeper;

use std::collections::HashMap;

use crate::platform::Platform;
use crate::world::{WinId, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game {
    /// Vista / 7 Minesweeper.
    Minesweeper,
    /// The grey XP one.
    MinesweeperXp,
    Solitaire,
    Spider,
    FreeCell,
    Hearts,
    ChessTitans,
    MahjongTitans,
    PurblePlace,
    /// Vista.
    InkBall,
    /// Vista Ultimate Extras.
    Tinker,
    HoldEm,
    Backgammon,
    Checkers,
    Spades,
    /// XP's 3D Pinball Space Cadet.
    Pinball,
}

impl Game {
    pub fn is_minesweeper(self) -> bool {
        matches!(self, Game::Minesweeper | Game::MinesweeperXp)
    }
}

/// Which game (if any) a window belongs to, from its exe name.
pub fn identify(exe: &str, title: &str) -> Option<Game> {
    let game = match exe {
        "minesweeper.exe" => Game::Minesweeper,
        "winmine.exe" => Game::MinesweeperXp,
        "solitaire.exe" | "sol.exe" => Game::Solitaire,
        "spidersolitaire.exe" | "spider.exe" => Game::Spider,
        "freecell.exe" => Game::FreeCell,
        "hearts.exe" | "mshearts.exe" => Game::Hearts,
        "chess.exe" => Game::ChessTitans,
        "mahjong.exe" => Game::MahjongTitans,
        "purbleplace.exe" => Game::PurblePlace,
        "inkball.exe" => Game::InkBall,
        "tinker.exe" => Game::Tinker,
        "bckgzm.exe" => Game::Backgammon,
        "chkrzm.exe" => Game::Checkers,
        "shvlzm.exe" => Game::Spades,
        "pinball.exe" => Game::Pinball,
        _ => {
            // Texas Hold 'Em (a Vista Ultimate Extra) shipped under a few names.
            let t = title.to_lowercase();
            if exe.contains("holdem") || exe.contains("texas") || t.contains("hold 'em") || t.contains("hold em") {
                Game::HoldEm
            } else {
                return None;
            }
        }
    };
    Some(game)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Won,
    Lost,
    /// Game ended, can't tell how.
    Over,
}

#[derive(Clone, Copy, Debug)]
pub enum GameEvent {
    /// A game was just opened.
    Opened(WinId, Game),
    /// A game's end-of-game dialog popped up.
    Finished(Game, Outcome),
}

/// Guess the result from the dialog's title ("Game Won", "Game Lost", ...).
/// None for dialogs that aren't about the game ending (Options etc.).
fn outcome(title: &str) -> Option<Outcome> {
    let t = title.to_lowercase();
    if ["won", "win", "congrat", "victory"].iter().any(|k| t.contains(k)) {
        Some(Outcome::Won)
    } else if ["lost", "lose", "defeat"].iter().any(|k| t.contains(k)) {
        Some(Outcome::Lost)
    } else if t.contains("over") {
        Some(Outcome::Over)
    } else {
        None
    }
}

struct Known {
    pid: u32,
    game: Option<Game>,
}

/// Keeps track of which open windows are games, and notices the popups
/// they show when a game is won or lost.
#[derive(Default)]
pub struct Watcher {
    known: HashMap<WinId, Known>,
    /// Main window of each running game, by process id.
    mains: HashMap<u32, WinId>,
    seen_dialogs: Vec<WinId>,
    /// The first scan only learns what's already open (no events).
    primed: bool,
}

fn area(world: &World, id: WinId) -> f32 {
    world.window(id).map(|w| (w.rect.r - w.rect.l) * (w.rect.b - w.rect.t)).unwrap_or(0.0)
}

impl Watcher {
    pub fn update(&mut self, plat: &dyn Platform, world: &World) -> Vec<GameEvent> {
        let mut events = Vec::new();
        self.known.retain(|id, _| world.window(*id).is_some());
        self.mains.retain(|_, id| world.window(*id).is_some());
        self.seen_dialogs.retain(|id| world.window(*id).is_some());

        for w in &world.windows {
            if self.known.contains_key(&w.id) {
                continue;
            }
            let title = plat.window_title(w.id);
            let (pid, exe) = plat.window_process(w.id).unwrap_or((0, String::new()));
            let game = identify(&exe, &title);
            self.known.insert(w.id, Known { pid, game });
            let Some(game) = game else { continue };
            match self.mains.get(&pid).copied() {
                None => {
                    self.mains.insert(pid, w.id);
                    if self.primed {
                        events.push(GameEvent::Opened(w.id, game));
                    }
                }
                Some(main) if area(world, w.id) > area(world, main) => {
                    // Bigger than what we thought was the main window.
                    self.mains.insert(pid, w.id);
                }
                Some(_) => {
                    // A smaller extra window from a running game: a dialog.
                    if self.primed && !self.seen_dialogs.contains(&w.id) {
                        self.seen_dialogs.push(w.id);
                        if let Some(o) = outcome(&title) {
                            events.push(GameEvent::Finished(game, o));
                        }
                    }
                }
            }
        }
        self.primed = true;
        events
    }

    /// The main window of a running game matching `want`.
    pub fn find(&self, want: impl Fn(Game) -> bool) -> Option<(WinId, Game)> {
        self.mains.values().find_map(|id| {
            let game = self.known.get(id)?.game?;
            want(game).then_some((*id, game))
        })
    }

    /// Is this the main window of a game?
    pub fn game_window(&self, id: WinId) -> Option<Game> {
        let k = self.known.get(&id)?;
        (self.mains.get(&k.pid) == Some(&id)).then_some(k.game).flatten()
    }
}
