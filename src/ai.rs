//! Chess engine: iterative-deepening alpha-beta with transposition table,
//! quiescence search, null-move pruning, killer/history move ordering.

use crate::chess::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const INF: i32 = 1_000_000;
const MATE: i32 = 100_000;

const VAL: [i32; 7] = [0, 100, 320, 330, 500, 900, 0];

#[rustfmt::skip]
const PST: [[i32; 64]; 7] = [
    [0; 64],
    // pawn
    [ 0,  0,  0,  0,  0,  0,  0,  0,
     50, 50, 50, 50, 50, 50, 50, 50,
     10, 10, 20, 30, 30, 20, 10, 10,
      5,  5, 10, 25, 25, 10,  5,  5,
      0,  0,  0, 20, 20,  0,  0,  0,
      5, -5,-10,  0,  0,-10, -5,  5,
      5, 10, 10,-20,-20, 10, 10,  5,
      0,  0,  0,  0,  0,  0,  0,  0],
    // knight
    [-50,-40,-30,-30,-30,-30,-40,-50,
     -40,-20,  0,  0,  0,  0,-20,-40,
     -30,  0, 10, 15, 15, 10,  0,-30,
     -30,  5, 15, 20, 20, 15,  5,-30,
     -30,  0, 15, 20, 20, 15,  0,-30,
     -30,  5, 10, 15, 15, 10,  5,-30,
     -40,-20,  0,  5,  5,  0,-20,-40,
     -50,-40,-30,-30,-30,-30,-40,-50],
    // bishop
    [-20,-10,-10,-10,-10,-10,-10,-20,
     -10,  0,  0,  0,  0,  0,  0,-10,
     -10,  0,  5, 10, 10,  5,  0,-10,
     -10,  5,  5, 10, 10,  5,  5,-10,
     -10,  0, 10, 10, 10, 10,  0,-10,
     -10, 10, 10, 10, 10, 10, 10,-10,
     -10,  5,  0,  0,  0,  0,  5,-10,
     -20,-10,-10,-10,-10,-10,-10,-20],
    // rook
    [  0,  0,  0,  0,  0,  0,  0,  0,
       5, 10, 10, 10, 10, 10, 10,  5,
      -5,  0,  0,  0,  0,  0,  0, -5,
      -5,  0,  0,  0,  0,  0,  0, -5,
      -5,  0,  0,  0,  0,  0,  0, -5,
      -5,  0,  0,  0,  0,  0,  0, -5,
      -5,  0,  0,  0,  0,  0,  0, -5,
       0,  0,  0,  5,  5,  0,  0,  0],
    // queen
    [-20,-10,-10, -5, -5,-10,-10,-20,
     -10,  0,  0,  0,  0,  0,  0,-10,
     -10,  0,  5,  5,  5,  5,  0,-10,
      -5,  0,  5,  5,  5,  5,  0, -5,
       0,  0,  5,  5,  5,  5,  0, -5,
     -10,  5,  5,  5,  5,  5,  0,-10,
     -10,  0,  5,  0,  0,  0,  0,-10,
     -20,-10,-10, -5, -5,-10,-10,-20],
    // king, middlegame
    [-30,-40,-40,-50,-50,-40,-40,-30,
     -30,-40,-40,-50,-50,-40,-40,-30,
     -30,-40,-40,-50,-50,-40,-40,-30,
     -30,-40,-40,-50,-50,-40,-40,-30,
     -20,-30,-30,-40,-40,-30,-30,-20,
     -10,-20,-20,-20,-20,-20,-20,-10,
      20, 20,  0,  0,  0,  0, 20, 20,
      20, 30, 10,  0,  0, 10, 30, 20],
];

#[rustfmt::skip]
const KING_END: [i32; 64] = [
    -50,-40,-30,-20,-20,-30,-40,-50,
    -30,-20,-10,  0,  0,-10,-20,-30,
    -30,-10, 20, 30, 30, 20,-10,-30,
    -30,-10, 30, 40, 40, 30,-10,-30,
    -30,-10, 30, 40, 40, 30,-10,-30,
    -30,-10, 20, 30, 30, 20,-10,-30,
    -30,-30,  0,  0,  0,  0,-30,-30,
    -50,-30,-30,-30,-30,-30,-30,-50];

/// Static evaluation from the side-to-move's point of view.
pub fn evaluate(p: &Pos) -> i32 {
    let mut mg = [0i32; 2];
    let mut eg = [0i32; 2];
    let mut phase = 0; // 0 = endgame .. 24 = opening
    let mut bishops = [0; 2];
    let mut pawn_files = [[0u8; 8]; 2];
    for s in 0..64u8 {
        let pc = p.at(s);
        if pc == 0 {
            continue;
        }
        let c = (color(pc) >> 3) as usize;
        let k = kind(pc) as usize;
        let idx = if c == 0 { (s ^ 56) as usize } else { s as usize };
        match k as u8 {
            KING => {
                mg[c] += PST[6][idx];
                eg[c] += KING_END[idx];
            }
            _ => {
                let v = VAL[k] + PST[k][idx];
                mg[c] += v;
                eg[c] += v;
            }
        }
        phase += [0, 0, 1, 1, 2, 4, 0][k];
        if k as u8 == BISHOP {
            bishops[c] += 1;
        }
        if k as u8 == PAWN {
            pawn_files[c][(s & 7) as usize] += 1;
        }
    }
    for c in 0..2 {
        if bishops[c] >= 2 {
            mg[c] += 30;
            eg[c] += 40;
        }
        for f in 0..8 {
            let n = pawn_files[c][f] as i32;
            if n > 1 {
                mg[c] -= 12 * (n - 1);
                eg[c] -= 20 * (n - 1);
            }
            if n > 0 {
                let l = if f > 0 { pawn_files[c][f - 1] } else { 0 };
                let r = if f < 7 { pawn_files[c][f + 1] } else { 0 };
                if l == 0 && r == 0 {
                    mg[c] -= 10 * n;
                    eg[c] -= 15 * n;
                }
            }
        }
    }
    let phase = phase.min(24);
    let w = ((mg[0] - mg[1]) * phase + (eg[0] - eg[1]) * (24 - phase)) / 24;
    if p.side == WHITE {
        w + 10
    } else {
        -w + 10
    }
}

#[derive(Clone, Copy, Default)]
struct TTEntry {
    key: u64,
    depth: i8,
    score: i32,
    flag: u8, // 0 exact, 1 lower, 2 upper
    mv: Move,
}

pub struct SearchResult {
    pub best: Option<Move>,
    pub score: i32,
    pub depth: u32,
    pub nodes: u64,
}

pub struct Searcher {
    tt: Vec<TTEntry>,
    killers: [[Move; 2]; 128],
    history: Box<[[i32; 64]; 64]>,
    nodes: u64,
    deadline: Instant,
    stop: Arc<AtomicBool>,
    aborted: bool,
    rep: Vec<u64>,
}

impl Searcher {
    pub fn new(stop: Arc<AtomicBool>) -> Searcher {
        Searcher {
            tt: vec![TTEntry::default(); 1 << 20],
            killers: [[Move::NULL; 2]; 128],
            history: Box::new([[0; 64]; 64]),
            nodes: 0,
            deadline: Instant::now(),
            stop,
            aborted: false,
            rep: Vec::new(),
        }
    }

    /// `history`: hashes of all earlier positions in the game (for repetition draws).
    pub fn search(&mut self, root: &Pos, history: &[u64], skill: &Skill) -> SearchResult {
        self.deadline = Instant::now() + skill.time;
        self.nodes = 0;
        self.aborted = false;
        self.rep = history.to_vec();
        let legal = root.legal_moves();
        let mut res = SearchResult { best: legal.first().copied(), score: 0, depth: 0, nodes: 0 };
        if legal.len() <= 1 {
            return res;
        }
        // weak levels see every root move with an exact score and then pick like a human would
        let exact = skill.temp > 0;
        let mut order: Vec<usize> = (0..legal.len()).collect();
        let mut last_scores: Vec<i32> = vec![0; legal.len()];
        for depth in 1..=skill.depth {
            let mut alpha = -INF;
            let mut best_i = order[0];
            let mut scores = vec![-INF; legal.len()];
            for &i in &order {
                let m = legal[i];
                let n = root.make(m);
                self.rep.push(root.hash);
                let mut sc;
                if alpha == -INF || exact {
                    sc = -self.negamax(&n, depth as i32 - 1, -INF, INF, 1);
                } else {
                    sc = -self.negamax(&n, depth as i32 - 1, -alpha - 1, -alpha, 1);
                    if sc > alpha && !self.aborted {
                        sc = -self.negamax(&n, depth as i32 - 1, -INF, -alpha, 1);
                    }
                }
                self.rep.pop();
                if self.aborted {
                    break;
                }
                scores[i] = sc;
                if sc > alpha {
                    alpha = sc;
                    best_i = i;
                }
            }
            if self.aborted {
                break;
            }
            res = SearchResult { best: Some(legal[best_i]), score: alpha, depth, nodes: self.nodes };
            last_scores = scores.clone();
            order.sort_by_key(|&i| -scores[i]);
            if alpha.abs() > MATE - 1000 && !exact {
                break;
            }
            if Instant::now() > self.deadline {
                break;
            }
        }
        res.nodes = self.nodes;
        if exact && res.depth > 0 {
            // an outright blunder now and then...
            if fastrand::f32() < skill.blunder {
                let i = fastrand::usize(..legal.len());
                res.best = Some(legal[i]);
                res.score = last_scores[i];
                return res;
            }
            // ...otherwise softmax over the move scores: the weaker, the more often a worse move
            let best = *last_scores.iter().max().unwrap();
            let w: Vec<f64> = last_scores
                .iter()
                .map(|&s| (((s - best).max(-3000)) as f64 / skill.temp as f64).exp())
                .collect();
            let total: f64 = w.iter().sum();
            let mut r = fastrand::f64() * total;
            for (i, wi) in w.iter().enumerate() {
                r -= wi;
                if r <= 0.0 {
                    res.best = Some(legal[i]);
                    res.score = last_scores[i];
                    break;
                }
            }
        }
        res
    }

    fn check_time(&mut self) {
        if self.nodes & 2047 == 0 && (Instant::now() > self.deadline || self.stop.load(Ordering::Relaxed)) {
            self.aborted = true;
        }
    }

    fn negamax(&mut self, p: &Pos, mut depth: i32, mut alpha: i32, beta: i32, ply: usize) -> i32 {
        self.nodes += 1;
        self.check_time();
        if self.aborted {
            return 0;
        }
        if p.halfmove >= 100 || self.rep.iter().rev().take(p.halfmove as usize + 1).any(|&h| h == p.hash) {
            return 0;
        }
        let in_check = p.in_check();
        if in_check {
            depth += 1;
        }
        if depth <= 0 {
            return self.quiesce(p, alpha, beta, ply);
        }
        let alpha0 = alpha;
        let slot = (p.hash as usize) & (self.tt.len() - 1);
        let te = self.tt[slot];
        let mut tt_move = Move::NULL;
        if te.key == p.hash {
            tt_move = te.mv;
            if te.depth as i32 >= depth {
                let s = te.score;
                match te.flag {
                    0 => return s,
                    1 if s >= beta => return s,
                    2 if s <= alpha => return s,
                    _ => {}
                }
            }
        }
        // null move pruning
        if !in_check && depth >= 3 && ply > 0 && has_pieces(p) && evaluate(p) >= beta {
            let n = p.make_null();
            self.rep.push(p.hash);
            let s = -self.negamax(&n, depth - 3, -beta, -beta + 1, ply + 1);
            self.rep.pop();
            if self.aborted {
                return 0;
            }
            if s >= beta {
                return beta;
            }
        }
        let mut moves = Vec::with_capacity(48);
        p.pseudo_moves(&mut moves, false);
        let mut scored: Vec<(i32, Move)> = moves.iter().map(|&m| (self.order_score(p, m, tt_move, ply), m)).collect();
        scored.sort_unstable_by_key(|x| -x.0);
        let mut best = -INF;
        let mut best_move = Move::NULL;
        let mut legal = 0;
        self.rep.push(p.hash);
        for (i, &(_, m)) in scored.iter().enumerate() {
            let n = p.make(m);
            if n.attacked(n.king[(p.side >> 3) as usize], n.side) {
                continue;
            }
            legal += 1;
            let quiet = !m.is_capture() && m.promo == 0;
            let mut s;
            // late move reduction
            if legal > 4 && depth >= 3 && quiet && !in_check && !n.in_check() && i > 3 {
                s = -self.negamax(&n, depth - 2, -alpha - 1, -alpha, ply + 1);
                if s > alpha {
                    s = -self.negamax(&n, depth - 1, -beta, -alpha, ply + 1);
                }
            } else if legal > 1 {
                s = -self.negamax(&n, depth - 1, -alpha - 1, -alpha, ply + 1);
                if s > alpha && s < beta {
                    s = -self.negamax(&n, depth - 1, -beta, -alpha, ply + 1);
                }
            } else {
                s = -self.negamax(&n, depth - 1, -beta, -alpha, ply + 1);
            }
            if self.aborted {
                self.rep.pop();
                return 0;
            }
            if s > best {
                best = s;
                best_move = m;
            }
            if s > alpha {
                alpha = s;
            }
            if alpha >= beta {
                if quiet {
                    let k = &mut self.killers[ply.min(127)];
                    if k[0] != m {
                        k[1] = k[0];
                        k[0] = m;
                    }
                    self.history[m.from as usize][m.to as usize] += depth * depth;
                }
                break;
            }
        }
        self.rep.pop();
        if legal == 0 {
            return if in_check { -MATE + ply as i32 } else { 0 };
        }
        let flag = if best <= alpha0 {
            2
        } else if best >= beta {
            1
        } else {
            0
        };
        self.tt[slot] = TTEntry { key: p.hash, depth: depth as i8, score: best, flag, mv: best_move };
        best
    }

    fn quiesce(&mut self, p: &Pos, mut alpha: i32, beta: i32, ply: usize) -> i32 {
        self.nodes += 1;
        self.check_time();
        if self.aborted {
            return 0;
        }
        let stand = evaluate(p);
        if stand >= beta {
            return stand;
        }
        if stand > alpha {
            alpha = stand;
        }
        if ply > 100 {
            return stand;
        }
        let mut moves = Vec::with_capacity(16);
        p.pseudo_moves(&mut moves, true);
        let mut scored: Vec<(i32, Move)> = moves.iter().map(|&m| (mvv_lva(p, m), m)).collect();
        scored.sort_unstable_by_key(|x| -x.0);
        for (_, m) in scored {
            // delta pruning
            let gain = VAL[kind(p.at(m.to)) as usize] + if m.promo != 0 { 800 } else { 0 };
            if stand + gain + 200 < alpha && m.flags & F_EP == 0 {
                continue;
            }
            let n = p.make(m);
            if n.attacked(n.king[(p.side >> 3) as usize], n.side) {
                continue;
            }
            let s = -self.quiesce(&n, -beta, -alpha, ply + 1);
            if self.aborted {
                return 0;
            }
            if s >= beta {
                return s;
            }
            if s > alpha {
                alpha = s;
            }
        }
        alpha
    }

    fn order_score(&self, p: &Pos, m: Move, tt: Move, ply: usize) -> i32 {
        if m == tt {
            return 10_000_000;
        }
        if m.is_capture() || m.promo != 0 {
            return 1_000_000 + mvv_lva(p, m);
        }
        let k = &self.killers[ply.min(127)];
        if m == k[0] {
            return 900_000;
        }
        if m == k[1] {
            return 800_000;
        }
        self.history[m.from as usize][m.to as usize]
    }
}

fn mvv_lva(p: &Pos, m: Move) -> i32 {
    let victim = if m.flags & F_EP != 0 { PAWN } else { kind(p.at(m.to)) };
    VAL[victim as usize] * 10 - VAL[kind(p.at(m.from)) as usize] / 10 + if m.promo == QUEEN { 9000 } else { 0 }
}

fn has_pieces(p: &Pos) -> bool {
    (0..64u8).any(|s| {
        let pc = p.at(s);
        pc != 0 && color(pc) == p.side && matches!(kind(pc), KNIGHT | BISHOP | ROOK | QUEEN)
    })
}

/// How the built-in engine plays at a given level.
#[derive(Clone, Copy, Debug)]
pub struct Skill {
    pub depth: u32,
    pub time: Duration,
    /// softmax temperature over root move scores, centipawns (0 = always the best move)
    pub temp: i32,
    /// chance of a completely random move
    pub blunder: f32,
}

pub const LEVELS: i32 = 20;

/// Difficulty level 1..=20, calibrated by self-play (`chess-rt --match A B N`, 24-40 games
/// per pair; adjacent levels measured +60..+290 Elo apart, typically ≈ +180):
///  1-14  search 3 plies, pick by softmax over move scores, temperature 450 → 15 cp
///        (÷1.3 per level); levels 1-4 also blunder now and then
///  15    best move at 3 plies;  16, 17, 18: best move at 4, 5, 6 plies
///  19-20 full iterative search for 0.4 s / 1.5 s
pub fn skill(level: i32) -> Skill {
    let ms = Duration::from_millis;
    let n = level.clamp(1, LEVELS);
    let fixed = |depth: u32| Skill { depth, time: ms(3000), temp: 0, blunder: 0.0 };
    match n {
        1..=14 => Skill {
            depth: 3,
            time: ms(3000),
            temp: (450.0 / 1.3f64.powi(n - 1)).round() as i32,
            blunder: match n {
                1 => 0.08,
                2 => 0.05,
                3 => 0.03,
                4 => 0.015,
                _ => 0.0,
            },
        },
        15 => fixed(3),
        16 => fixed(4),
        17 => fixed(5),
        18 => fixed(6),
        19 => Skill { depth: 64, time: ms(400), temp: 0, blunder: 0.0 },
        _ => Skill { depth: 64, time: ms(1500), temp: 0, blunder: 0.0 },
    }
}

/// Approximate strength of each level on Stockfish's UCI_Elo scale, from matches against
/// Stockfish 19 / Fairy-Stockfish with limited strength (16 games per anchor):
/// L1 ≈ 550 (vs Fairy 500: 56%), L6 ≈ 1150 (vs Fairy 760: 91%), L12 ≈ 1460 (vs SF 1320: 69%),
/// L17 ≈ 1660 (vs SF 1320: 88%), L20 ≈ 2100 (vs SF 1693: 91%, vs SF 1973: 69%); the rest interpolated.
pub const LEVEL_ELO: [i32; 20] =
    [550, 650, 760, 870, 1000, 1150, 1210, 1260, 1310, 1360, 1410, 1460, 1500, 1540, 1580, 1620, 1660, 1800, 1950, 2100];

/// Short description of a built-in level for the UI.
pub fn level_label(level: i32) -> String {
    let elo = LEVEL_ELO[(level.clamp(1, LEVELS) - 1) as usize];
    let kind = match elo {
        0..=799 => "новичок",
        800..=1199 => "любитель",
        1200..=1499 => "уверенный любитель",
        1500..=1799 => "сильный любитель",
        _ => "мастер клуба",
    };
    format!("{kind}, ≈{elo} Elo")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_mate_in_one() {
        // white: Qh5xf7#
        let p = Pos::from_fen("r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4").unwrap();
        let mut s = Searcher::new(Arc::new(AtomicBool::new(false)));
        let r = s.search(&p, &[], &Skill { depth: 4, time: Duration::from_secs(5), temp: 0, blunder: 0.0 });
        assert_eq!(r.best.unwrap().uci(), "h5f7");
    }

    #[test]
    fn takes_free_queen() {
        let p = Pos::from_fen("4k3/8/8/3q4/8/8/3R4/4K3 w - - 0 1").unwrap();
        let mut s = Searcher::new(Arc::new(AtomicBool::new(false)));
        let r = s.search(&p, &[], &Skill { depth: 4, time: Duration::from_secs(5), temp: 0, blunder: 0.0 });
        assert_eq!(r.best.unwrap().uci(), "d2d5");
    }
}
