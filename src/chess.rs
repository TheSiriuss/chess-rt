//! Chess rules: board, legal move generation, make move, SAN, game end detection.
//! Squares: a1 = 0, b1 = 1, ..., h8 = 63. Pieces: kind | colour (WHITE = 0, BLACK = 8).

pub const WHITE: u8 = 0;
pub const BLACK: u8 = 8;
pub const PAWN: u8 = 1;
pub const KNIGHT: u8 = 2;
pub const BISHOP: u8 = 3;
pub const ROOK: u8 = 4;
pub const QUEEN: u8 = 5;
pub const KING: u8 = 6;

pub const F_CAPTURE: u8 = 1;
pub const F_EP: u8 = 2;
pub const F_CASTLE: u8 = 4;
pub const F_DOUBLE: u8 = 8;

pub const NO_SQ: u8 = 64;

#[inline]
pub fn kind(p: u8) -> u8 {
    p & 7
}
#[inline]
pub fn color(p: u8) -> u8 {
    p & 8
}
#[inline]
pub fn file_of(s: u8) -> i8 {
    (s & 7) as i8
}
#[inline]
pub fn rank_of(s: u8) -> i8 {
    (s >> 3) as i8
}
#[inline]
fn sq(f: i8, r: i8) -> Option<u8> {
    if (0..8).contains(&f) && (0..8).contains(&r) {
        Some((r * 8 + f) as u8)
    } else {
        None
    }
}

pub fn sq_name(s: u8) -> String {
    format!("{}{}", (b'a' + (s & 7)) as char, (s >> 3) + 1)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Move {
    pub from: u8,
    pub to: u8,
    pub promo: u8,
    pub flags: u8,
}

impl Move {
    pub const NULL: Move = Move { from: 0, to: 0, promo: 0, flags: 0 };
    pub fn is_capture(&self) -> bool {
        self.flags & F_CAPTURE != 0
    }
    pub fn uci(&self) -> String {
        let mut s = format!("{}{}", sq_name(self.from), sq_name(self.to));
        if self.promo != 0 {
            s.push(b" pnbrqk"[self.promo as usize] as char);
        }
        s
    }
}

const KNIGHT_D: [(i8, i8); 8] = [(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)];
const KING_D: [(i8, i8); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
const DIAG_D: [(i8, i8); 4] = [(1, 1), (-1, 1), (-1, -1), (1, -1)];
const ORTH_D: [(i8, i8); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

// ---------- Zobrist keys (compile-time splitmix64) ----------
const fn splitmix(mut x: u64) -> (u64, u64) {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (x, z ^ (z >> 31))
}
struct Zob {
    piece: [[u64; 64]; 16],
    side: u64,
    castle: [u64; 16],
    ep: [u64; 8],
}
const fn gen_zob() -> Zob {
    let mut z = Zob { piece: [[0; 64]; 16], side: 0, castle: [0; 16], ep: [0; 8] };
    let mut st = 0x1234_5678_9ABC_DEF0u64;
    let mut p = 0;
    while p < 16 {
        let mut s = 0;
        while s < 64 {
            let (n, v) = splitmix(st);
            st = n;
            z.piece[p][s] = v;
            s += 1;
        }
        p += 1;
    }
    let (n, v) = splitmix(st);
    st = n;
    z.side = v;
    let mut i = 0;
    while i < 16 {
        let (n, v) = splitmix(st);
        st = n;
        z.castle[i] = v;
        i += 1;
    }
    i = 0;
    while i < 8 {
        let (n, v) = splitmix(st);
        st = n;
        z.ep[i] = v;
        i += 1;
    }
    z
}
static ZOB: Zob = gen_zob();

#[derive(Clone, Copy, Debug)]
pub struct Pos {
    pub sq: [u8; 64],
    pub side: u8,
    /// 1 = white O-O, 2 = white O-O-O, 4 = black O-O, 8 = black O-O-O
    pub castle: u8,
    pub ep: u8,
    pub halfmove: u16,
    pub fullmove: u16,
    pub hash: u64,
    pub king: [u8; 2],
}

pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

impl Pos {
    pub fn start() -> Pos {
        Pos::from_fen(START_FEN).unwrap()
    }

    pub fn from_fen(fen: &str) -> Option<Pos> {
        let mut it = fen.split_whitespace();
        let board = it.next()?;
        let mut p = Pos { sq: [0; 64], side: WHITE, castle: 0, ep: NO_SQ, halfmove: 0, fullmove: 1, hash: 0, king: [0; 2] };
        let mut r: i8 = 7;
        let mut f: i8 = 0;
        for c in board.chars() {
            match c {
                '/' => {
                    r -= 1;
                    f = 0;
                }
                '1'..='8' => f += c as i8 - b'0' as i8,
                _ => {
                    let col = if c.is_ascii_uppercase() { WHITE } else { BLACK };
                    let k = match c.to_ascii_lowercase() {
                        'p' => PAWN,
                        'n' => KNIGHT,
                        'b' => BISHOP,
                        'r' => ROOK,
                        'q' => QUEEN,
                        'k' => KING,
                        _ => return None,
                    };
                    let s = sq(f, r)?;
                    p.sq[s as usize] = k | col;
                    f += 1;
                }
            }
        }
        p.side = if it.next()? == "b" { BLACK } else { WHITE };
        for c in it.next().unwrap_or("-").chars() {
            p.castle |= match c {
                'K' => 1,
                'Q' => 2,
                'k' => 4,
                'q' => 8,
                _ => 0,
            };
        }
        let ep = it.next().unwrap_or("-");
        if ep != "-" {
            let b = ep.as_bytes();
            p.ep = sq(b[0] as i8 - b'a' as i8, b[1] as i8 - b'1' as i8)?;
        }
        p.halfmove = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        p.fullmove = it.next().and_then(|s| s.parse().ok()).unwrap_or(1);
        for s in 0..64u8 {
            let pc = p.sq[s as usize];
            if kind(pc) == KING {
                p.king[(color(pc) >> 3) as usize] = s;
            }
        }
        p.hash = p.compute_hash();
        Some(p)
    }

    pub fn compute_hash(&self) -> u64 {
        let mut h = 0u64;
        for s in 0..64 {
            let pc = self.sq[s];
            if pc != 0 {
                h ^= ZOB.piece[pc as usize][s];
            }
        }
        if self.side == BLACK {
            h ^= ZOB.side;
        }
        h ^= ZOB.castle[self.castle as usize];
        if self.ep != NO_SQ {
            h ^= ZOB.ep[(self.ep & 7) as usize];
        }
        h
    }

    #[inline]
    pub fn at(&self, s: u8) -> u8 {
        self.sq[s as usize]
    }

    /// Is square `s` attacked by pieces of colour `by`?
    pub fn attacked(&self, s: u8, by: u8) -> bool {
        let f = file_of(s);
        let r = rank_of(s);
        // pawns
        let pr = if by == WHITE { r - 1 } else { r + 1 };
        for df in [-1, 1] {
            if let Some(t) = sq(f + df, pr) {
                if self.at(t) == PAWN | by {
                    return true;
                }
            }
        }
        for (df, dr) in KNIGHT_D {
            if let Some(t) = sq(f + df, r + dr) {
                if self.at(t) == KNIGHT | by {
                    return true;
                }
            }
        }
        for (df, dr) in KING_D {
            if let Some(t) = sq(f + df, r + dr) {
                if self.at(t) == KING | by {
                    return true;
                }
            }
        }
        for (dirs, a, b) in [(&DIAG_D, BISHOP, QUEEN), (&ORTH_D, ROOK, QUEEN)] {
            for &(df, dr) in dirs.iter() {
                let (mut cf, mut cr) = (f + df, r + dr);
                while let Some(t) = sq(cf, cr) {
                    let pc = self.at(t);
                    if pc != 0 {
                        if pc == a | by || pc == b | by {
                            return true;
                        }
                        break;
                    }
                    cf += df;
                    cr += dr;
                }
            }
        }
        false
    }

    pub fn in_check(&self) -> bool {
        let k = self.king[(self.side >> 3) as usize];
        self.attacked(k, self.side ^ 8)
    }

    /// Pseudo-legal moves (may leave own king in check). Castling is fully checked.
    pub fn pseudo_moves(&self, out: &mut Vec<Move>, captures_only: bool) {
        let us = self.side;
        let them = us ^ 8;
        for s in 0..64u8 {
            let pc = self.at(s);
            if pc == 0 || color(pc) != us {
                continue;
            }
            let f = file_of(s);
            let r = rank_of(s);
            match kind(pc) {
                PAWN => {
                    let dir: i8 = if us == WHITE { 1 } else { -1 };
                    let start_rank = if us == WHITE { 1 } else { 6 };
                    let last_rank = if us == WHITE { 7 } else { 0 };
                    let push = |out: &mut Vec<Move>, to: u8, flags: u8| {
                        if rank_of(to) == last_rank {
                            for pr in [QUEEN, KNIGHT, ROOK, BISHOP] {
                                out.push(Move { from: s, to, promo: pr, flags });
                            }
                        } else {
                            out.push(Move { from: s, to, promo: 0, flags });
                        }
                    };
                    if let Some(t) = sq(f, r + dir) {
                        if self.at(t) == 0 {
                            if !captures_only || rank_of(t) == last_rank {
                                push(out, t, 0);
                            }
                            if !captures_only && r == start_rank {
                                let t2 = sq(f, r + 2 * dir).unwrap();
                                if self.at(t2) == 0 {
                                    out.push(Move { from: s, to: t2, promo: 0, flags: F_DOUBLE });
                                }
                            }
                        }
                    }
                    for df in [-1, 1] {
                        if let Some(t) = sq(f + df, r + dir) {
                            let tp = self.at(t);
                            if tp != 0 && color(tp) == them {
                                push(out, t, F_CAPTURE);
                            } else if t == self.ep {
                                out.push(Move { from: s, to: t, promo: 0, flags: F_CAPTURE | F_EP });
                            }
                        }
                    }
                }
                KNIGHT | KING => {
                    let d = if kind(pc) == KNIGHT { &KNIGHT_D } else { &KING_D };
                    for &(df, dr) in d.iter() {
                        if let Some(t) = sq(f + df, r + dr) {
                            let tp = self.at(t);
                            if tp == 0 {
                                if !captures_only {
                                    out.push(Move { from: s, to: t, promo: 0, flags: 0 });
                                }
                            } else if color(tp) == them {
                                out.push(Move { from: s, to: t, promo: 0, flags: F_CAPTURE });
                            }
                        }
                    }
                    if kind(pc) == KING && !captures_only {
                        self.castle_moves(s, out);
                    }
                }
                k => {
                    let dirs: &[(i8, i8)] = match k {
                        BISHOP => &DIAG_D,
                        ROOK => &ORTH_D,
                        _ => &KING_D,
                    };
                    for &(df, dr) in dirs {
                        let (mut cf, mut cr) = (f + df, r + dr);
                        while let Some(t) = sq(cf, cr) {
                            let tp = self.at(t);
                            if tp == 0 {
                                if !captures_only {
                                    out.push(Move { from: s, to: t, promo: 0, flags: 0 });
                                }
                            } else {
                                if color(tp) == them {
                                    out.push(Move { from: s, to: t, promo: 0, flags: F_CAPTURE });
                                }
                                break;
                            }
                            cf += df;
                            cr += dr;
                        }
                    }
                }
            }
        }
    }

    fn castle_moves(&self, k: u8, out: &mut Vec<Move>) {
        let us = self.side;
        let them = us ^ 8;
        let (home, ks, qs) = if us == WHITE { (4u8, 1u8, 2u8) } else { (60u8, 4u8, 8u8) };
        if k != home || self.attacked(k, them) {
            return;
        }
        if self.castle & ks != 0
            && self.at(home + 1) == 0
            && self.at(home + 2) == 0
            && self.at(home + 3) == ROOK | us
            && !self.attacked(home + 1, them)
            && !self.attacked(home + 2, them)
        {
            out.push(Move { from: home, to: home + 2, promo: 0, flags: F_CASTLE });
        }
        if self.castle & qs != 0
            && self.at(home - 1) == 0
            && self.at(home - 2) == 0
            && self.at(home - 3) == 0
            && self.at(home - 4) == ROOK | us
            && !self.attacked(home - 1, them)
            && !self.attacked(home - 2, them)
        {
            out.push(Move { from: home, to: home - 2, promo: 0, flags: F_CASTLE });
        }
    }

    /// Apply a (pseudo-legal) move, returning the new position.
    pub fn make(&self, m: Move) -> Pos {
        let mut p = *self;
        let us = self.side;
        let pc = p.sq[m.from as usize];
        p.sq[m.from as usize] = 0;
        if m.flags & F_EP != 0 {
            let cap = if us == WHITE { m.to - 8 } else { m.to + 8 };
            p.sq[cap as usize] = 0;
        }
        p.sq[m.to as usize] = if m.promo != 0 { m.promo | us } else { pc };
        if m.flags & F_CASTLE != 0 {
            let (rf, rt) = if m.to > m.from { (m.from + 3, m.from + 1) } else { (m.from - 4, m.from - 1) };
            p.sq[rt as usize] = p.sq[rf as usize];
            p.sq[rf as usize] = 0;
        }
        if kind(pc) == KING {
            p.king[(us >> 3) as usize] = m.to;
            p.castle &= if us == WHITE { !3 } else { !12 };
        }
        for (corner, right) in [(0u8, 2u8), (7, 1), (56, 8), (63, 4)] {
            if m.from == corner || m.to == corner {
                p.castle &= !right;
            }
        }
        p.ep = if m.flags & F_DOUBLE != 0 { (m.from + m.to) / 2 } else { NO_SQ };
        p.halfmove = if kind(pc) == PAWN || m.is_capture() { 0 } else { p.halfmove + 1 };
        if us == BLACK {
            p.fullmove += 1;
        }
        p.side = us ^ 8;
        p.hash = p.compute_hash();
        p
    }

    /// Null move (for search pruning).
    pub fn make_null(&self) -> Pos {
        let mut p = *self;
        p.side ^= 8;
        p.ep = NO_SQ;
        p.hash = p.compute_hash();
        p
    }

    pub fn legal_moves(&self) -> Vec<Move> {
        let mut v = Vec::with_capacity(64);
        self.pseudo_moves(&mut v, false);
        v.retain(|&m| self.is_legal_after(m));
        v
    }

    /// Does this pseudo-legal move leave our king safe?
    pub fn is_legal_after(&self, m: Move) -> bool {
        let n = self.make(m);
        !n.attacked(n.king[(self.side >> 3) as usize], n.side)
    }

    pub fn insufficient_material(&self) -> bool {
        let mut minors = Vec::new();
        for s in 0..64u8 {
            let pc = self.at(s);
            match kind(pc) {
                0 | KING => {}
                KNIGHT | BISHOP => minors.push((kind(pc), s)),
                _ => return false,
            }
        }
        match minors.len() {
            0 | 1 => true,
            _ => {
                // only bishops, all on same colour squares
                minors.iter().all(|&(k, _)| k == BISHOP) && {
                    let c0 = (file_of(minors[0].1) + rank_of(minors[0].1)) & 1;
                    minors.iter().all(|&(_, s)| (file_of(s) + rank_of(s)) & 1 == c0)
                }
            }
        }
    }

    /// Standard algebraic notation with Russian piece letters (Кр Ф Л С К).
    pub fn san(&self, m: Move, legal: &[Move]) -> String {
        let pc = self.at(m.from);
        let mut s = String::new();
        if m.flags & F_CASTLE != 0 {
            s.push_str(if m.to > m.from { "O-O" } else { "O-O-O" });
        } else {
            let k = kind(pc);
            if k == PAWN {
                if m.is_capture() {
                    s.push((b'a' + (m.from & 7)) as char);
                    s.push('x');
                }
                s.push_str(&sq_name(m.to));
                if m.promo != 0 {
                    s.push('=');
                    s.push_str(piece_letter(m.promo));
                }
            } else {
                s.push_str(piece_letter(k));
                let others: Vec<&Move> = legal
                    .iter()
                    .filter(|o| o.to == m.to && o.from != m.from && self.at(o.from) == pc)
                    .collect();
                if !others.is_empty() {
                    let same_file = others.iter().any(|o| file_of(o.from) == file_of(m.from));
                    let same_rank = others.iter().any(|o| rank_of(o.from) == rank_of(m.from));
                    if !same_file {
                        s.push((b'a' + (m.from & 7)) as char);
                    } else if !same_rank {
                        s.push((b'1' + (m.from >> 3)) as char);
                    } else {
                        s.push_str(&sq_name(m.from));
                    }
                }
                if m.is_capture() {
                    s.push('x');
                }
                s.push_str(&sq_name(m.to));
            }
        }
        let n = self.make(m);
        if n.in_check() {
            s.push(if n.legal_moves().is_empty() { '#' } else { '+' });
        }
        s
    }
}

pub fn piece_letter(k: u8) -> &'static str {
    match k {
        KNIGHT => "К",
        BISHOP => "С",
        ROOK => "Л",
        QUEEN => "Ф",
        KING => "Кр",
        _ => "",
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Checkmate { winner: u8 },
    Stalemate,
    FiftyMoves,
    Repetition,
    Material,
}

impl Outcome {
    pub fn text(&self) -> String {
        match self {
            Outcome::Checkmate { winner } => {
                format!("Мат! Победили {}", if *winner == WHITE { "белые" } else { "чёрные" })
            }
            Outcome::Stalemate => "Пат — ничья".into(),
            Outcome::FiftyMoves => "Ничья: правило 50 ходов".into(),
            Outcome::Repetition => "Ничья: троекратное повторение".into(),
            Outcome::Material => "Ничья: недостаточно материала".into(),
        }
    }
}

/// A game: positions after each move, so undo is trivial.
#[derive(Clone)]
pub struct Game {
    pub positions: Vec<Pos>,
    pub moves: Vec<Move>,
    pub sans: Vec<String>,
}

impl Game {
    pub fn new() -> Game {
        Game { positions: vec![Pos::start()], moves: vec![], sans: vec![] }
    }
    pub fn pos(&self) -> &Pos {
        self.positions.last().unwrap()
    }
    pub fn play(&mut self, m: Move) {
        let p = *self.pos();
        let legal = p.legal_moves();
        self.sans.push(p.san(m, &legal));
        self.positions.push(p.make(m));
        self.moves.push(m);
    }
    pub fn undo(&mut self) -> bool {
        if self.moves.is_empty() {
            return false;
        }
        self.positions.pop();
        self.moves.pop();
        self.sans.pop();
        true
    }
    pub fn outcome(&self) -> Option<Outcome> {
        let p = self.pos();
        if p.legal_moves().is_empty() {
            return Some(if p.in_check() { Outcome::Checkmate { winner: p.side ^ 8 } } else { Outcome::Stalemate });
        }
        if p.halfmove >= 100 {
            return Some(Outcome::FiftyMoves);
        }
        if self.positions.iter().filter(|q| q.hash == p.hash).count() >= 3 {
            return Some(Outcome::Repetition);
        }
        if p.insufficient_material() {
            return Some(Outcome::Material);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perft(p: &Pos, d: u32) -> u64 {
        if d == 0 {
            return 1;
        }
        let mut v = Vec::new();
        p.pseudo_moves(&mut v, false);
        let mut n = 0;
        for m in v {
            if p.is_legal_after(m) {
                n += if d == 1 { 1 } else { perft(&p.make(m), d - 1) };
            }
        }
        n
    }

    #[test]
    fn perft_start() {
        let p = Pos::start();
        assert_eq!(perft(&p, 1), 20);
        assert_eq!(perft(&p, 2), 400);
        assert_eq!(perft(&p, 3), 8902);
        assert_eq!(perft(&p, 4), 197281);
    }

    #[test]
    fn perft_kiwipete() {
        let p = Pos::from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1").unwrap();
        assert_eq!(perft(&p, 1), 48);
        assert_eq!(perft(&p, 2), 2039);
        assert_eq!(perft(&p, 3), 97862);
    }

    #[test]
    fn perft_pos3_4_5() {
        let p = Pos::from_fen("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1").unwrap();
        assert_eq!(perft(&p, 4), 43238);
        let p = Pos::from_fen("r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1").unwrap();
        assert_eq!(perft(&p, 3), 9467);
        let p = Pos::from_fen("rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8").unwrap();
        assert_eq!(perft(&p, 3), 62379);
    }

    #[test]
    fn san_and_mate() {
        let mut g = Game::new();
        for uci in ["f2f3", "e7e5", "g2g4", "d8h4"] {
            let m = g.pos().legal_moves().into_iter().find(|m| m.uci() == uci).unwrap();
            g.play(m);
        }
        assert_eq!(g.sans, vec!["f3", "e5", "g4", "Фh4#"]);
        assert_eq!(g.outcome(), Some(Outcome::Checkmate { winner: BLACK }));
    }

    #[test]
    fn hash_incremental_matches() {
        let p = Pos::start();
        let m = p.legal_moves()[3];
        let n = p.make(m);
        assert_eq!(n.hash, n.compute_hash());
    }
}
