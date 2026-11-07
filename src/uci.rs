//! Minimal UCI client: start an engine process, set its strength, ask for moves.

use crate::chess::Move;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct UciOption {
    pub name: String,
    pub kind: String,
    pub min: i64,
    pub max: i64,
}

pub struct Engine {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    pub name: String,
    pub options: Vec<UciOption>,
    applied_level: Option<i32>,
    /// last "info ... score" seen during a search: (centipawns or mate*100000, depth)
    pub last_score: Option<(i32, u32)>,
}

/// The strength slider (1..=20) mapped onto an engine. 20 = full strength.
pub fn level_elo(level: i32, lo: i64, hi: i64) -> i64 {
    let top = hi.min(3000).max(lo);
    lo + (top - lo) * (level.clamp(1, 19) as i64 - 1) / 18
}
pub fn level_nodes(level: i32) -> u64 {
    (100.0 * 1.65f64.powi(level.clamp(1, 19) - 1)) as u64
}

/// Human description of a level for an engine whose strength control is `kind`.
pub fn level_text(kind: &str, lo: i64, hi: i64, level: i32) -> String {
    if level >= 20 {
        return "полная сила, 1.5 с на ход".into();
    }
    match kind {
        "elo" => format!("ограничен до ≈{} Elo", level_elo(level, lo, hi)),
        "skill" => format!("уровень мастерства {} из 20", level - 1),
        _ => format!("перебор до {} позиций на ход", level_nodes(level)),
    }
}

impl Engine {
    pub fn start(path: &Path) -> Result<Engine, String> {
        let mut child = Command::new(path)
            .current_dir(path.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("не удалось запустить: {e}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut e = Engine { child, stdin, rx, name: String::new(), options: vec![], applied_level: None, last_score: None };
        e.send("uci")?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let line = e.recv_until(deadline)?;
            if let Some(n) = line.strip_prefix("id name ") {
                e.name = n.trim().to_string();
            } else if line.starts_with("option name ") {
                e.options.push(parse_option(&line));
            } else if line.trim() == "uciok" {
                break;
            }
        }
        if e.has("Threads") {
            e.send("setoption name Threads value 1")?;
        }
        if e.has("Hash") {
            e.send("setoption name Hash value 64")?;
        }
        e.ready()?;
        Ok(e)
    }

    fn has(&self, name: &str) -> bool {
        self.options.iter().any(|o| o.name.eq_ignore_ascii_case(name))
    }
    fn opt(&self, name: &str) -> Option<&UciOption> {
        self.options.iter().find(|o| o.name.eq_ignore_ascii_case(name))
    }

    pub fn send(&mut self, s: &str) -> Result<(), String> {
        writeln!(self.stdin, "{s}").and_then(|_| self.stdin.flush()).map_err(|e| format!("движок не отвечает: {e}"))
    }

    fn recv_until(&mut self, deadline: Instant) -> Result<String, String> {
        let left = deadline.saturating_duration_since(Instant::now());
        match self.rx.recv_timeout(left) {
            Ok(l) => Ok(l),
            Err(RecvTimeoutError::Timeout) => Err("движок не ответил вовремя".into()),
            Err(RecvTimeoutError::Disconnected) => Err("движок завершился".into()),
        }
    }

    pub fn ready(&mut self) -> Result<(), String> {
        self.send("isready")?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if self.recv_until(deadline)?.trim() == "readyok" {
                return Ok(());
            }
        }
    }

    /// How a strength level maps onto this engine, for the UI.
    pub fn strength_kind(&self) -> &'static str {
        if self.has("UCI_Elo") && self.has("UCI_LimitStrength") {
            "elo"
        } else if self.has("Skill Level") {
            "skill"
        } else {
            "nodes"
        }
    }

    pub fn elo_range(&self) -> Option<(i64, i64)> {
        self.opt("UCI_Elo").map(|o| (o.min, o.max))
    }

    fn apply_level(&mut self, level: i32) -> Result<(), String> {
        if self.applied_level == Some(level) {
            return Ok(());
        }
        match self.strength_kind() {
            "elo" => {
                if level >= 20 {
                    self.send("setoption name UCI_LimitStrength value false")?;
                } else {
                    let (lo, hi) = self.elo_range().unwrap_or((1320, 3190));
                    self.send("setoption name UCI_LimitStrength value true")?;
                    self.send(&format!("setoption name UCI_Elo value {}", level_elo(level, lo, hi)))?;
                }
            }
            "skill" => {
                let o = self.opt("Skill Level").cloned().unwrap_or_default();
                let v = if level >= 20 { o.max } else { (level - 1) as i64 };
                self.send(&format!("setoption name Skill Level value {}", v.clamp(o.min, o.max.max(o.min))))?;
            }
            _ => {}
        }
        self.applied_level = Some(level);
        self.ready()
    }

    /// Ask for a move after `moves` from the start position. `stop` aborts early (returns None).
    pub fn best_move_cancellable(&mut self, moves: &[Move], level: i32, stop: &AtomicBool) -> Result<Option<String>, String> {
        self.apply_level(level)?;
        let mut pos = String::from("position startpos");
        if !moves.is_empty() {
            pos.push_str(" moves");
            for m in moves {
                pos.push(' ');
                pos.push_str(&m.uci());
            }
        }
        self.send(&pos)?;
        let go = if level >= 20 {
            "go movetime 1500".to_string()
        } else if self.strength_kind() == "nodes" {
            format!("go nodes {}", level_nodes(level))
        } else {
            "go movetime 400".to_string()
        };
        self.send(&go)?;
        self.last_score = None;
        let mut stopped = false;
        let hard = Instant::now() + Duration::from_secs(30);
        loop {
            if !stopped && stop.load(Ordering::Relaxed) {
                self.send("stop")?;
                stopped = true;
            }
            match self.rx.recv_timeout(Duration::from_millis(40)) {
                Ok(line) => {
                    if let Some(rest) = line.strip_prefix("bestmove") {
                        let mv = rest.split_whitespace().next().unwrap_or("").to_string();
                        return Ok(if stopped || mv.is_empty() || mv == "(none)" { None } else { Some(mv) });
                    }
                    if line.starts_with("info") {
                        if let Some(s) = parse_score(&line) {
                            self.last_score = Some(s);
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() > hard {
                        self.send("stop")?;
                        stopped = true;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Err("движок завершился".into()),
            }
        }
    }

    pub fn best_move(&mut self, moves: &[Move], level: i32) -> Result<Option<String>, String> {
        self.best_move_cancellable(moves, level, &AtomicBool::new(false))
    }

    pub fn new_game(&mut self) -> Result<(), String> {
        self.send("ucinewgame")?;
        self.ready()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.send("quit");
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(300) {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn parse_option(line: &str) -> UciOption {
    // option name <name...> type <t> [default x] [min a] [max b] [var ...]
    let rest = &line["option name ".len()..];
    let (name, tail) = match rest.find(" type ") {
        Some(i) => (&rest[..i], &rest[i + 6..]),
        None => (rest, ""),
    };
    let mut o = UciOption { name: name.trim().to_string(), ..Default::default() };
    let toks: Vec<&str> = tail.split_whitespace().collect();
    o.kind = toks.first().unwrap_or(&"").to_string();
    for w in toks.windows(2) {
        match w[0] {
            "min" => o.min = w[1].parse().unwrap_or(0),
            "max" => o.max = w[1].parse().unwrap_or(0),
            _ => {}
        }
    }
    o
}

fn parse_score(line: &str) -> Option<(i32, u32)> {
    let t: Vec<&str> = line.split_whitespace().collect();
    let mut depth = 0;
    let mut score = None;
    for i in 0..t.len() {
        match t[i] {
            "depth" => depth = t.get(i + 1)?.parse().unwrap_or(0),
            "score" => {
                let kind = *t.get(i + 1)?;
                let v: i32 = t.get(i + 2)?.parse().ok()?;
                score = Some(if kind == "mate" { v.signum() * 100_000 } else { v });
            }
            _ => {}
        }
    }
    score.map(|s| (s, depth))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_options_and_scores() {
        let o = parse_option("option name UCI_Elo type spin default 1320 min 1320 max 3190");
        assert_eq!((o.name.as_str(), o.kind.as_str(), o.min, o.max), ("UCI_Elo", "spin", 1320, 3190));
        let o = parse_option("option name Skill Level type spin default 20 min 0 max 20");
        assert_eq!(o.name, "Skill Level");
        assert_eq!(parse_score("info depth 12 seldepth 18 score cp -35 nodes 1000 pv e2e4"), Some((-35, 12)));
        assert_eq!(parse_score("info depth 5 score mate 3 pv a1a2"), Some((100000, 5)));
    }
}
