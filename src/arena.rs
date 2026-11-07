//! Headless matches for calibrating difficulty: `chess-rt --match A B N`.
//! A player is a built-in level ("3") or an installed UCI engine with a strength ("stockfish@1400").

use crate::ai;
use crate::chess::*;
use crate::uci;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub enum Player {
    Builtin(i32),
    /// custom built-in settings, e.g. "d3t60b0.01" (depth, temperature, blunder rate)
    Custom(ai::Skill),
    Engine { id: String, level: i32 },
}

impl Player {
    pub fn parse(s: &str) -> Player {
        match s.split_once('@') {
            Some((id, l)) => Player::Engine { id: id.to_string(), level: l.parse().unwrap_or(1) },
            None if s.starts_with('d') => {
                let num = |key: char| -> f32 {
                    let i = s.find(key).map(|i| i + 1).unwrap_or(s.len());
                    s[i..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<String>().parse().unwrap_or(0.0)
                };
                let mut k = ai::skill(8);
                k.depth = num('d') as u32;
                k.temp = num('t') as i32;
                k.blunder = num('b');
                Player::Custom(k)
            }
            None => Player::Builtin(s.parse().unwrap_or(1)),
        }
    }
    fn name(&self) -> String {
        match self {
            Player::Builtin(l) => format!("встроенный {l}"),
            Player::Custom(k) => format!("d{}t{}b{}", k.depth, k.temp, k.blunder),
            Player::Engine { id, level } => format!("{id}@{level}"),
        }
    }
}

enum Live {
    Builtin(ai::Skill, ai::Searcher),
    Engine(uci::Engine, i32),
}

impl Live {
    fn new(p: &Player) -> Live {
        match p {
            Player::Builtin(l) => Live::Builtin(ai::skill(*l), ai::Searcher::new(Arc::new(AtomicBool::new(false)))),
            Player::Custom(k) => Live::Builtin(*k, ai::Searcher::new(Arc::new(AtomicBool::new(false)))),
            Player::Engine { id, level } => {
                let path = crate::engines::installed_path(id).expect("engine not installed");
                let e = uci::Engine::start(&path).expect("engine failed to start");
                Live::Engine(e, *level)
            }
        }
    }
    fn pick(&mut self, g: &Game) -> Option<Move> {
        match self {
            Live::Builtin(l, s) => {
                let n = g.positions.len();
                let hist: Vec<u64> = g.positions[..n - 1].iter().map(|p| p.hash).collect();
                s.search(g.pos(), &hist, l).best
            }
            Live::Engine(e, level) => e.best_move(&g.moves, *level).ok().flatten().and_then(|u| {
                g.pos().legal_moves().into_iter().find(|m| m.uci() == u)
            }),
        }
    }
}

/// Plays one game; returns the score for `a` (1, 0.5, 0).
fn play(a: &Player, b: &Player, a_white: bool, seed: u64) -> f32 {
    fastrand::seed(seed);
    let mut g = Game::new();
    // two random plies so the games differ
    for _ in 0..2 {
        let m = g.pos().legal_moves();
        g.play(m[fastrand::usize(..m.len())]);
    }
    let mut pa = Live::new(a);
    let mut pb = Live::new(b);
    for _ in 0..220 {
        if let Some(o) = g.outcome() {
            return match o {
                Outcome::Checkmate { winner } => {
                    if (winner == WHITE) == a_white {
                        1.0
                    } else {
                        0.0
                    }
                }
                _ => 0.5,
            };
        }
        let a_to_move = (g.pos().side == WHITE) == a_white;
        let m = if a_to_move { pa.pick(&g) } else { pb.pick(&g) };
        match m {
            Some(m) => g.play(m),
            None => return if a_to_move { 0.0 } else { 1.0 },
        }
    }
    0.5
}

pub fn run(a: &str, b: &str, games: usize) {
    let (a, b) = (Player::parse(a), Player::parse(b));
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(games.max(1));
    let next = Arc::new(Mutex::new(0usize));
    let total = Arc::new(Mutex::new((0.0f32, 0usize, 0usize, 0usize))); // score, w, d, l
    let mut hs = vec![];
    for _ in 0..threads {
        let (a, b, next, total) = (a.clone(), b.clone(), next.clone(), total.clone());
        hs.push(std::thread::spawn(move || loop {
            let i = {
                let mut n = next.lock().unwrap();
                if *n >= games {
                    return;
                }
                *n += 1;
                *n - 1
            };
            let r = play(&a, &b, i % 2 == 0, 1000 + i as u64);
            let mut t = total.lock().unwrap();
            t.0 += r;
            if r == 1.0 {
                t.1 += 1
            } else if r == 0.5 {
                t.2 += 1
            } else {
                t.3 += 1
            }
        }));
    }
    for h in hs {
        let _ = h.join();
    }
    let t = total.lock().unwrap();
    let p = t.0 / games as f32;
    let elo = if p > 0.0 && p < 1.0 { -400.0 * (1.0 / p - 1.0).log10() } else if p >= 1.0 { 999.0 } else { -999.0 };
    println!(
        "{} vs {}: {:.1}/{} ({:.0}%)  +{} ={} -{}  ≈ {:+.0} Elo",
        a.name(),
        b.name(),
        t.0,
        games,
        p * 100.0,
        t.1,
        t.2,
        t.3,
        elo
    );
}
