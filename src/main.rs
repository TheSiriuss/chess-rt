mod ai;
mod arena;
mod audio;
mod chess;
mod engines;
mod pond;
mod render;
mod uci;

use chess::*;
use pond::Pond;
use render::{Renderer, Uniforms};
use slint::{ComponentHandle, GraphicsAPI, Model, ModelRc, RenderingState, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

slint::include_modules!();

const MK_SEL: i32 = 1;
const MK_MOVE: i32 = 2;
const MK_CAPT: i32 = 4;
const MK_LAST: i32 = 8;
const MK_CHECK: i32 = 16;
const MK_HOVER: i32 = 32;
const PH: f32 = 0.20;

const FLIGHT: f64 = 0.6; // captured piece flying into the pond
const SINK: f64 = 1.8; // ...and sinking to the bottom

/// Pieces lost by each side, in order: [white pieces, black pieces].
fn captured_lists(g: &Game) -> [Vec<u8>; 2] {
    let mut out = [vec![], vec![]];
    for (i, m) in g.moves.iter().enumerate() {
        if m.is_capture() {
            let p = &g.positions[i];
            let cs = if m.flags & F_EP != 0 {
                if p.side == WHITE { m.to - 8 } else { m.to + 8 }
            } else {
                m.to
            };
            let pc = p.at(cs);
            out[(color(pc) >> 3) as usize].push(pc);
        }
    }
    out
}

#[derive(Clone, Copy)]
struct Anim {
    piece: u8,
    from: (f32, f32),
    to: (f32, f32),
    t0: f64,
    dur: f64,
    hide: u8,
    lift0: f32,
}

#[derive(Clone, Copy)]
struct CapAnim {
    piece: u8,
    square: u8,
    side: usize,
    index: usize,
    t0: f64,
    splashed: bool,
}

#[derive(Clone, Copy)]
struct Drag {
    from: u8,
    press: (f32, f32),
    pos: (f32, f32),
    active: bool,
}

struct AiResult {
    gen: u64,
    mv: Option<Move>,
    /// move in UCI notation from an external engine (resolved on the UI thread)
    uci: Option<String>,
    score: i32,
    depth: u32,
    side: u8,
    err: Option<String>,
}

/// A request for an external UCI engine, served by one long-lived worker thread.
struct EngineReq {
    gen: u64,
    id: String,
    moves: Vec<Move>,
    level: i32,
    side: u8,
    stop: Arc<AtomicBool>,
}

fn engine_worker(rx: mpsc::Receiver<EngineReq>, tx: mpsc::Sender<AiResult>) {
    let mut cur: Option<(String, uci::Engine)> = None;
    for req in rx {
        let fail = |err: String| AiResult { gen: req.gen, mv: None, uci: None, score: 0, depth: 0, side: req.side, err: Some(err) };
        if cur.as_ref().map(|c| c.0 != req.id).unwrap_or(true) {
            cur = None;
            let started = engines::installed_path(&req.id)
                .ok_or_else(|| "движок не установлен".to_string())
                .and_then(|p| uci::Engine::start(&p));
            match started {
                Ok(e) => cur = Some((req.id.clone(), e)),
                Err(e) => {
                    let _ = tx.send(fail(e));
                    continue;
                }
            }
        }
        let e = &mut cur.as_mut().unwrap().1;
        if req.moves.len() <= 1 {
            let _ = e.new_game();
        }
        match e.best_move_cancellable(&req.moves, req.level, &req.stop) {
            Ok(mv) => {
                let (score, depth) = e.last_score.unwrap_or((0, 0));
                let _ = tx.send(AiResult { gen: req.gen, mv: None, uci: mv, score, depth, side: req.side, err: None });
            }
            Err(err) => {
                cur = None;
                let _ = tx.send(fail(err));
            }
        }
    }
}

// ---- settings kept between runs: ~/.config/chess-rt/settings.txt
fn settings_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("chess-rt/settings.txt")
}

fn save_settings(app: &App, opponent_id: &str) {
    let b = |v: bool| if v { "1" } else { "0" };
    let txt = format!(
        "daycycle={}\ndaylen={:.1}\nlevel={}\nopponent={}\nmode={}\nnight={}\nrain={}\nmusic={:.2}\nsfx={:.2}\nquality={}\nshadows={}\nao={}\nbloom={}\nspec={}\nsoftness={:.2}\n",
        b(app.get_opt_daycycle()),
        app.get_opt_daylen(),
        app.get_level(),
        opponent_id,
        app.get_mode(),
        b(app.global::<Theme>().get_night()),
        b(app.get_opt_rain()),
        app.get_music_volume(),
        app.get_sfx_volume(),
        app.get_opt_quality(),
        b(app.get_opt_shadows()),
        b(app.get_opt_ao()),
        b(app.get_opt_bloom()),
        b(app.get_opt_spec()),
        app.get_opt_softness(),
    );
    let p = settings_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, txt);
}

/// Returns the saved opponent id.
fn load_settings(app: &App) -> String {
    let txt = std::fs::read_to_string(settings_path()).unwrap_or_default();
    let mut opponent = String::new();
    for line in txt.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let f: f32 = v.parse().unwrap_or(0.0);
        let on = v == "1";
        match k {
            "daycycle" => app.set_opt_daycycle(on),
            "daylen" => app.set_opt_daylen(f.clamp(2.0, 30.0)),
            "level" => app.set_level((f as i32).clamp(1, ai::LEVELS)),
            "opponent" => opponent = v.to_string(),
            "mode" => app.set_mode((f as i32).clamp(0, 3)),
            "night" => app.global::<Theme>().set_night(on),
            "rain" => app.set_opt_rain(on),
            "music" => app.set_music_volume(f.clamp(0.0, 1.0)),
            "sfx" => app.set_sfx_volume(f.clamp(0.0, 1.0)),
            "quality" => app.set_opt_quality((f as i32).clamp(0, 2)),
            "shadows" => app.set_opt_shadows(on),
            "ao" => app.set_opt_ao(on),
            "bloom" => app.set_opt_bloom(on),
            "spec" => app.set_opt_spec(on),
            "softness" => app.set_opt_softness(f.clamp(0.0, 1.0)),
            _ => {}
        }
    }
    opponent
}

struct State {
    audio: Option<audio::Audio>,
    mode: i32,
    /// opponent ids by combo index; "" = built-in
    opponents: Vec<String>,
    engine_tx: mpsc::Sender<EngineReq>,
    downloads: Vec<(&'static str, engines::DlState)>,
    notice: Option<String>,
    game: Game,
    flip: bool,
    sel: Option<u8>,
    sel_moves: Vec<Move>,
    hover: Option<u8>,
    mouse: Option<(f32, f32)>,
    drag: Option<Drag>,
    anims: Vec<Anim>,
    cap: Option<CapAnim>,
    lift: f32,
    pond: Pond,
    /// day cycle position in days (fraction = time of day), the night state we last showed, clock
    day_t: f64,
    night_ui: bool,
    clock: String,
    promo: Option<(u8, u8)>,
    ai_gen: u64,
    ai_busy: bool,
    ai_started: f64,
    ai_stop: Arc<AtomicBool>,
    ai_tx: mpsc::Sender<AiResult>,
    ai_pending: Option<AiResult>,
    start: Instant,
    last_t: f64,
    eval: Option<(i32, u32)>,
    // fps
    frames: u32,
    fps_t0: f64,
    fps: f32,
}

impl State {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
    fn pos(&self) -> &Pos {
        self.game.pos()
    }

    /// square -> screen board coords of its centre (board units, y up)
    fn center(&self, s: u8) -> (f32, f32) {
        let (f, r) = (file_of(s) as f32, rank_of(s) as f32);
        if self.flip {
            (7.0 - f + 0.5, 7.0 - r + 0.5)
        } else {
            (f + 0.5, r + 0.5)
        }
    }
    fn screen_index(&self, s: u8) -> usize {
        let (x, y) = self.center(s);
        (y as usize) * 8 + x as usize
    }
    fn square_at(&self, b: (f32, f32)) -> Option<u8> {
        if !(0.0..8.0).contains(&b.0) || !(0.0..8.0).contains(&b.1) {
            return None;
        }
        let (x, y) = (b.0 as u8, b.1 as u8);
        Some(if self.flip { (7 - y) * 8 + (7 - x) } else { y * 8 + x })
    }

    fn outcome(&self) -> Option<Outcome> {
        self.game.outcome()
    }

    fn is_ai_turn(&self, mode: i32) -> bool {
        let side = self.pos().side;
        match mode {
            0 => side == BLACK,
            1 => side == WHITE,
            2 => false,
            _ => true,
        }
    }

    fn human_can_move(&self, mode: i32) -> bool {
        self.promo.is_none() && !self.ai_busy && self.ai_pending.is_none() && !self.is_ai_turn(mode) && self.outcome().is_none()
    }

    fn sfx(&self, e: audio::Sfx, vol: f32, x: f32, delay: f32) {
        if let Some(a) = &self.audio {
            a.play_in(e, vol, ((x - 4.0) / 6.0).clamp(-0.8, 0.8), delay);
        }
    }

    fn select(&mut self, s: Option<u8>) {
        if let Some(sq) = s {
            if self.sel != s {
                let x = self.center(sq).0;
                self.sfx(audio::Sfx::Lift, 0.8, x, 0.0);
            }
        }
        self.sel = s;
        self.sel_moves = match s {
            Some(s) => self.pos().legal_moves().into_iter().filter(|m| m.from == s).collect(),
            None => vec![],
        };
        if s.is_none() {
            self.lift = 0.0;
        }
    }

    fn cancel_ai(&mut self) {
        self.ai_stop.store(true, Ordering::Relaxed);
        self.ai_stop = Arc::new(AtomicBool::new(false));
        self.ai_gen += 1;
        self.ai_busy = false;
        self.ai_pending = None;
    }

    /// Play a legal move with animations. `drop` = where the dragged piece was released.
    fn apply_move(&mut self, m: Move, drop: Option<(f32, f32)>) {
        let now = self.now();
        let p = *self.pos();
        let piece = p.at(m.from);
        self.anims.clear();
        let (from, dur, lift0) = match drop {
            Some(d) => (d, 0.10, 0.32),
            None => (self.center(m.from), 0.30, if self.sel == Some(m.from) { self.lift } else { 0.0 }),
        };
        self.anims.push(Anim { piece, from, to: self.center(m.to), t0: now, dur, hide: m.to, lift0 });
        if m.flags & F_CASTLE != 0 {
            let (rf, rt) = if m.to > m.from { (m.from + 3, m.from + 1) } else { (m.from - 4, m.from - 1) };
            self.anims.push(Anim {
                piece: p.at(rf),
                from: self.center(rf),
                to: self.center(rt),
                t0: now + 0.06,
                dur: 0.30,
                hide: rt,
                lift0: 0.0,
            });
        }
        if m.is_capture() {
            let cs = if m.flags & F_EP != 0 {
                if p.side == WHITE {
                    m.to - 8
                } else {
                    m.to + 8
                }
            } else {
                m.to
            };
            let pc = p.at(cs);
            let side = (color(pc) >> 3) as usize;
            let index = captured_lists(&self.game)[side].len();
            self.cap = Some(CapAnim { piece: pc, square: cs, side, index, t0: now + dur * 0.5, splashed: false });
        }
        self.game.play(m);
        // sounds: the knock when the piece lands, then check / promotion / the end of the game
        let land = self.center(m.to).0;
        self.sfx(if m.is_capture() { audio::Sfx::Capture } else { audio::Sfx::Move }, 1.0, land, dur as f32);
        if m.promo != 0 {
            self.sfx(audio::Sfx::Promote, 0.8, land, dur as f32 + 0.05);
        }
        if let Some(o) = self.outcome() {
            let human = match self.mode {
                0 => Some(WHITE),
                1 => Some(BLACK),
                _ => None,
            };
            let e = match (o, human) {
                (Outcome::Checkmate { winner }, Some(h)) if winner != h => audio::Sfx::Lose,
                (Outcome::Checkmate { .. }, _) => audio::Sfx::Win,
                _ => audio::Sfx::Draw,
            };
            self.sfx(e, 1.0, 4.0, dur as f32 + 0.35);
        } else if self.pos().in_check() {
            let k = self.pos().king[(self.pos().side >> 3) as usize];
            let x = self.center(k).0;
            self.sfx(audio::Sfx::Check, 0.9, x, dur as f32 + 0.08);
        }
        if std::env::var("CHESS_LOG").is_ok() {
            eprintln!("{} {}", self.game.moves.len(), self.game.sans.last().unwrap());
        }
        self.select(None);
        self.drag = None;
        self.promo = None;
    }

    /// Moves wait for this; a piece already sinking in the pond doesn't block play.
    fn animating(&self) -> bool {
        !self.anims.is_empty() || self.cap.map(|c| !c.splashed).unwrap_or(false)
    }

    fn start_ai(&mut self, level: i32, opponent: usize) {
        if self.ai_busy || self.outcome().is_some() {
            return;
        }
        self.ai_busy = true;
        self.ai_started = self.now();
        let gen = self.ai_gen;
        let pos = *self.pos();
        if let Some(id) = self.opponents.get(opponent).filter(|id| !id.is_empty()) {
            let _ = self.engine_tx.send(EngineReq {
                gen,
                id: id.clone(),
                moves: self.game.moves.clone(),
                level,
                side: pos.side,
                stop: self.ai_stop.clone(),
            });
            return;
        }
        let n = self.game.positions.len();
        let hist: Vec<u64> = self.game.positions[..n - 1].iter().map(|p| p.hash).collect();
        let stop = self.ai_stop.clone();
        let tx = self.ai_tx.clone();
        std::thread::spawn(move || {
            let mut s = ai::Searcher::new(stop);
            let r = s.search(&pos, &hist, &ai::skill(level));
            let _ = tx.send(AiResult { gen, mv: r.best, uci: None, score: r.score, depth: r.depth, side: pos.side, err: None });
        });
    }

    fn uniforms(&mut self, app: &App) -> Uniforms {
        let t = self.now();
        let dt = (t - self.last_t).clamp(0.0, 0.1) as f32;
        self.last_t = t;
        let sf = app.window().scale_factor();
        let board = [app.get_bx() * sf, app.get_by() * sf, app.get_bsize() * sf];

        // fps
        self.frames += 1;
        if t - self.fps_t0 > 0.5 {
            self.fps = self.frames as f32 / (t - self.fps_t0) as f32;
            self.frames = 0;
            self.fps_t0 = t;
        }

        let pos = *self.pos();
        let mut hidden = [false; 64];
        let mut movers = [[0.0f32; 4]; 4];
        let mut mscale = [1.0f32; 4];
        let mut nm = 0;
        let mut top_z = PH + 0.01;
        let tcode = |pc: u8| -> f32 { (kind(pc) + if color(pc) == BLACK { 6 } else { 0 }) as f32 };

        for a in &self.anims {
            let k = (((t - a.t0) / a.dur).clamp(0.0, 1.0)) as f32;
            let e = k * k * (3.0 - 2.0 * k);
            let x = a.from.0 + (a.to.0 - a.from.0) * e;
            let y = a.from.1 + (a.to.1 - a.from.1) * e;
            let lift = a.lift0 * (1.0 - e) + (std::f32::consts::PI * k).sin() * if a.dur > 0.2 { 0.22 } else { 0.0 };
            hidden[a.hide as usize] = true;
            if nm < 3 {
                movers[nm] = [x, y, lift, tcode(a.piece)];
                mscale[nm] = 1.0 + lift * 0.35;
                top_z = top_z.max(lift + PH * mscale[nm] + 0.01);
                nm += 1;
            }
        }
        // selected piece floats; dragged piece follows the cursor
        let target_lift = if self.drag.map(|d| d.active).unwrap_or(false) {
            0.32
        } else if self.sel.is_some() {
            0.10
        } else {
            0.0
        };
        self.lift += (target_lift - self.lift) * (1.0 - (-dt * 14.0).exp());
        if let Some(s) = self.sel {
            let snapping = self.anims.iter().any(|a| a.hide == s);
            if nm < 3 && pos.at(s) != 0 && !snapping {
                hidden[s as usize] = true;
                let c = match self.drag {
                    Some(d) if d.active => d.pos,
                    _ => self.center(s),
                };
                let lift = self.lift;
                movers[nm] = [c.0, c.1, lift, tcode(pos.at(s))];
                mscale[nm] = 1.0 + lift * 0.35;
                top_z = top_z.max(lift + PH * mscale[nm] + 0.01);
                nm += 1;
            }
        }
        let _ = nm;

        let mut pieces = [0i32; 64];
        for s in 0..64u8 {
            let pc = pos.at(s);
            if pc != 0 && !hidden[s as usize] {
                pieces[self.screen_index(s)] = tcode(pc) as i32;
            }
        }

        let mut marks = [0i32; 64];
        if let Some(m) = self.game.moves.last() {
            marks[self.screen_index(m.from)] |= MK_LAST;
            marks[self.screen_index(m.to)] |= MK_LAST;
        }
        if let Some(s) = self.sel {
            marks[self.screen_index(s)] |= MK_SEL;
            for m in &self.sel_moves {
                marks[self.screen_index(m.to)] |= if m.is_capture() { MK_CAPT } else { MK_MOVE };
            }
        }
        if let Some(h) = self.hover {
            marks[self.screen_index(h)] |= MK_HOVER;
        }
        let in_check = pos.in_check();
        let king_sq = pos.king[(pos.side >> 3) as usize];
        if in_check {
            marks[self.screen_index(king_sq)] |= MK_CHECK;
        }

        // ---- the pond: captured piece flight, sunken pieces, life, weather
        let tf = t as f32;
        let lost = captured_lists(&self.game);
        let mut sunk = [0i32; 32];
        let mut sink_slot = -1;
        let mut sink_t = 0.0;
        let mut flying: Option<(usize, usize)> = None;
        if let Some(c) = self.cap {
            let k = ((t - c.t0) / FLIGHT).clamp(0.0, 1.0) as f32;
            if !c.splashed {
                flying = Some((c.side, c.index));
                let from = self.center(c.square);
                let to = pond::slot_centre(c.side, c.index);
                let e = k * k * (3.0 - 2.0 * k);
                let lift = (std::f32::consts::PI * k).sin() * 0.9;
                movers[3] = [from.0 + (to.0 - from.0) * e, from.1 + (to.1 - from.1) * e, lift, tcode(c.piece)];
                mscale[3] = 1.0 + lift * 0.35;
                top_z = top_z.max(lift + PH * mscale[3] + 0.01);
            } else {
                sink_slot = (c.side * 16 + c.index) as i32;
                sink_t = ((t - c.t0 - FLIGHT) / SINK).clamp(0.0, 1.0) as f32;
            }
        }
        for side in 0..2 {
            for (i, &pc) in lost[side].iter().enumerate().take(16) {
                if flying == Some((side, i)) {
                    continue;
                }
                sunk[side * 16 + i] = tcode(pc) as i32;
            }
        }
        // visible water, in board units
        let sqv = app.get_bsize() / 8.0;
        let h_log = app.window().size().height as f32 / sf;
        let x0 = -app.get_bx() / sqv;
        let x1 = (app.get_area_w() - app.get_bx()) / sqv;
        let y1 = 8.0 + app.get_by() / sqv;
        let y0 = 8.0 - (h_log - app.get_by()) / sqv;
        self.pond.set_bounds([x0, x1, y0, y1]);
        let mix3 = |a: [f32; 3], b: [f32; 3], k: f32| [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k];
        let norm = |v: [f32; 3]| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / l, v[1] / l, v[2] / l]
        };
        let rain = self.pond.rain;
        let (sun_dir, sun_col, sun_i, night_target, dusk);
        if app.get_opt_daycycle() {
            // ---- day cycle: tod 0 = midnight, 0.25 sunrise, 0.5 noon, 0.75 sunset
            let len = (app.get_opt_daylen().max(1.0) * 60.0) as f64;
            self.day_t += dt as f64 / len;
            let tod = self.day_t.rem_euclid(1.0) as f32;
            let s = (std::f32::consts::TAU * (tod - 0.25)).sin(); // > 0 while the sun is up
            // the sun (or the moon, half a day later) crosses the sky from left to right
            let path = |ang: f32, el: f32| {
                let a = std::f32::consts::PI * ang.clamp(0.0, 1.0);
                let xy = [-a.cos(), 0.45 + 0.55 * a.sin()];
                let l = (xy[0] * xy[0] + xy[1] * xy[1]).sqrt();
                let el = el.max(0.10); // never fully flat: the shadows stay finite
                norm([xy[0] / l * el.cos(), xy[1] / l * el.cos(), el.sin()])
            };
            let k = |x: f32| x.clamp(0.0, 1.0) * x.clamp(0.0, 1.0) * (3.0 - 2.0 * x.clamp(0.0, 1.0));
            if s >= 0.0 {
                sun_dir = path((tod - 0.25) / 0.5, s * 1.08);
                // golden hour: bright and orange almost down to the horizon
                sun_col = mix3([1.0, 0.55, 0.28], [1.0, 0.93, 0.82], k(s / 0.45));
                sun_i = 4.6 * (1.0 - 0.55 * rain) * k(s / 0.07);
            } else {
                sun_dir = path(((tod + 0.5).rem_euclid(1.0) - 0.25) / 0.5, -s * 1.0);
                sun_col = [0.50, 0.62, 1.0];
                sun_i = 0.55 * (1.0 - 0.5 * rain) * k(-s / 0.25);
            }
            night_target = k((0.06 - s) / 0.18);
            dusk = (-(s / 0.22) * (s / 0.22)).exp() * (1.0 - 0.7 * night_target);
            self.clock = format!("{:02}:{:02}", (tod * 24.0) as u32 % 24, (tod * 1440.0) as u32 % 60);
            // the UI's night look follows the sky; a manual toggle jumps to the next dusk / dawn
            let ui_night = app.global::<Theme>().get_night();
            if ui_night != self.night_ui {
                let base = self.day_t.floor();
                self.day_t = if ui_night { base + 0.80 } else { base + 1.27 };
                self.night_ui = ui_night;
            } else if (night_target > 0.5) != ui_night {
                self.night_ui = night_target > 0.5;
                app.global::<Theme>().set_night(self.night_ui);
            }
        } else {
            let n = if app.global::<Theme>().get_night() { 1.0 } else { 0.0 };
            night_target = n;
            let night = self.pond.night;
            sun_dir = norm(mix3(norm([-0.55, 0.62, 0.56]), norm([0.45, 0.55, 0.70]), night));
            sun_col = mix3([1.0, 0.93, 0.82], [0.50, 0.62, 1.0], night);
            sun_i = (4.6 * (1.0 - 0.55 * rain)) * (1.0 - night) + (0.55 * (1.0 - 0.5 * rain)) * night;
            dusk = 0.0;
            self.clock.clear();
            self.night_ui = n > 0.5;
        }
        self.pond.update(tf, dt, night_target, app.get_opt_rain());
        let night = self.pond.night;
        let (pl, pl_col, n_pl, n_lantern) = self.pond.lights(tf);
        let (fish, fish_k, n_fish) = self.pond.fish_uniforms();

        let pulse = (t as f32 * 3.0).sin() * 0.1 + 0.9;
        let (glow0, glow0_col) = match self.sel {
            Some(s) => {
                let c = match self.drag {
                    Some(d) if d.active => d.pos,
                    _ => self.center(s),
                };
                ([c.0, c.1, 0.45 + self.lift, 4.0 * pulse], [1.0, 0.62, 0.28])
            }
            None => ([0.0; 4], [0.0; 3]),
        };
        let (glow1, glow1_col) = if in_check {
            let c = self.center(king_sq);
            ([c.0, c.1, 0.5, 6.0 * (0.75 + 0.25 * (t as f32 * 6.0).sin())], [1.0, 0.12, 0.06])
        } else {
            ([0.0; 4], [0.0; 3])
        };

        let soft = app.get_opt_softness();
        Uniforms {
            board,
            time: tf,
            pieces,
            marks,
            movers,
            mover_scale: mscale,
            opt: [
                app.get_opt_shadows() as i32 as f32,
                40.0 * (1.0 - soft) + 3.0, // penumbra sharpness k
                app.get_opt_ao() as i32 as f32,
                app.get_opt_spec() as i32 as f32,
            ],
            glow0,
            glow0_col,
            glow1,
            glow1_col,
            top_z,
            bloom: if app.get_opt_bloom() { 1.0 } else { 0.0 },
            exposure: 1.0 + 0.3 * night,
            scale: match app.get_opt_quality() {
                0 => 1.0,
                1 => 0.75,
                _ => 0.5,
            },
            sun: [sun_dir[0], sun_dir[1], sun_dir[2], sun_i],
            sun_col,
            night,
            rain,
            flash: self.pond.flash,
            dusk,
            pl,
            pl_col,
            n_pl,
            n_lantern,
            fish,
            fish_k,
            n_fish,
            rip: self.pond.ripple_uniforms(),
            food: self.pond.food_uniforms(tf),
            sunk,
            sink_slot,
            sink_t,
        }
    }

    /// Something on screen is moving -> keep redrawing.
    fn needs_frames(&self, app: &App) -> bool {
        let _ = app;
        self.animating() || self.cap.is_some() || self.drag.is_some() || self.sel.is_some() || self.lift > 0.001
    }
}

fn sync_ui(app: &App, st: &State) {
    let pos = st.pos();
    let white = pos.side == WHITE;
    app.set_white_to_move(white);
    app.set_thinking(st.ai_busy);
    let mode = app.get_mode();
    let out = st.outcome();
    app.set_game_over(out.map(|o| o.text()).unwrap_or_default().into());
    app.set_status(if out.is_some() { "Партия окончена" } else if white { "Ход белых" } else { "Ход чёрных" }.into());
    let sub = if out.is_none() && pos.in_check() {
        "Шах!".to_string()
    } else {
        match mode {
            0 => "вы играете белыми".into(),
            1 => "вы играете чёрными".into(),
            2 => "игра вдвоём".into(),
            _ => "компьютер против компьютера".into(),
        }
    };
    app.set_substatus(st.notice.clone().unwrap_or(sub).into());
    let level = app.get_level();
    let opp = st.opponents.get(app.get_opponent() as usize).cloned().unwrap_or_default();
    let lt = if opp.is_empty() {
        format!("встроенный ИИ · {}", ai::level_label(level))
    } else {
        let (kind, lo, hi) = engines::installed_strength(&opp);
        uci::level_text(&kind, lo, hi, level)
    };
    app.set_level_text(lt.into());
    app.set_eval_text(
        st.eval
            .map(|(s, d)| {
                if s.abs() > 90_000 {
                    format!("мат\nглуб. {d}")
                } else {
                    format!("{:+.2}\nглуб. {d}", s as f32 / 100.0)
                }
            })
            .unwrap_or_default()
            .into(),
    );
    // move list
    let mut rows = Vec::new();
    let n = st.game.sans.len();
    for (i, pair) in st.game.sans.chunks(2).enumerate() {
        rows.push(MoveRow {
            num: i as i32 + 1,
            white: pair[0].clone().into(),
            black: pair.get(1).cloned().unwrap_or_default().into(),
            white_cur: i * 2 + 1 == n,
            black_cur: i * 2 + 2 == n,
        });
    }
    let model = app.get_moves();
    let same_len = model.row_count() == rows.len();
    if let (true, Some(vm)) = (same_len, model.as_any().downcast_ref::<VecModel<MoveRow>>()) {
        for (i, r) in rows.into_iter().enumerate() {
            if vm.row_data(i).as_ref() != Some(&r) {
                vm.set_row_data(i, r);
            }
        }
    } else {
        app.set_moves(ModelRc::new(VecModel::from(rows)));
    }
    let files: Vec<SharedString> =
        (0..8).map(|i| ((b'a' + if st.flip { 7 - i } else { i }) as char).to_string().into()).collect();
    let ranks: Vec<SharedString> =
        (0..8).map(|i| ((b'1' + if st.flip { i } else { 7 - i }) as char).to_string().into()).collect();
    app.set_file_labels(ModelRc::new(VecModel::from(files)));
    app.set_rank_labels(ModelRc::new(VecModel::from(ranks)));
    app.set_promo_visible(st.promo.is_some());
    app.set_move_count(st.game.sans.len() as i32);
    app.set_promo_white(white);
    let hover_piece = st
        .hover
        .map(|h| {
            let pc = pos.at(h);
            (pc != 0 && color(pc) == pos.side) || st.sel_moves.iter().any(|m| m.to == h)
        })
        .unwrap_or(false);
    app.set_board_hover_piece(hover_piece && st.human_can_move(mode));
}

/// Rebuild the opponent list from installed engines, keeping the current choice if possible.
fn refresh_opponents(app: &App, st: &mut State) {
    let cur = st.opponents.get(app.get_opponent() as usize).cloned().unwrap_or_default();
    st.opponents = vec![String::new()];
    let mut names: Vec<SharedString> = vec!["Встроенный ИИ".into()];
    for id in engines::installed_ids() {
        st.opponents.push(id.to_string());
        let n = engines::installed_name(id).unwrap_or_else(|| engines::entry(id).map(|e| e.name).unwrap_or(id).to_string());
        names.push(n.into());
    }
    app.set_opponent_names(ModelRc::new(VecModel::from(names)));
    let idx = st.opponents.iter().position(|o| *o == cur).unwrap_or(0);
    app.set_opponent(idx as i32);
}

fn refresh_engine_rows(app: &App, st: &State) {
    let rows: Vec<EngineRow> = engines::CATALOG
        .iter()
        .map(|e| {
            let dl = st.downloads.iter().find(|d| d.0 == e.id).map(|d| d.1.lock().unwrap().0.clone());
            let installed = engines::installed_path(e.id).is_some();
            let (status, progress, busy) = match dl {
                Some(engines::Dl::Looking) => ("ищу последнюю версию…".to_string(), -1.0, true),
                Some(engines::Dl::Downloading { got, total }) => (
                    format!("загрузка {:.1} / {:.1} МБ", got as f64 / 1e6, total as f64 / 1e6),
                    if total > 0 { got as f32 / total as f32 } else { 0.0 },
                    true,
                ),
                Some(engines::Dl::Unpacking) => ("распаковка…".into(), 1.0, true),
                Some(engines::Dl::Checking) => ("проверка протокола UCI…".into(), 1.0, true),
                Some(engines::Dl::Failed(m)) if !installed => (format!("ошибка: {m}"), -1.0, false),
                _ if installed => (
                    format!("установлен · {}", engines::installed_name(e.id).unwrap_or_default()),
                    -1.0,
                    false,
                ),
                _ => ("не установлен".into(), -1.0, false),
            };
            EngineRow {
                id: e.id.into(),
                name: e.name.into(),
                about: e.about.into(),
                status: status.into(),
                progress,
                installed,
                busy,
            }
        })
        .collect();
    app.set_engine_rows(ModelRc::new(VecModel::from(rows)));
}

fn main() -> Result<(), slint::PlatformError> {
    // headless calibration: chess-rt --match A B N   (A/B = level "3" or engine "stockfish@2")
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 4 && args[1] == "--match" {
        arena::run(&args[2], &args[3], args.get(4).and_then(|n| n.parse().ok()).unwrap_or(20));
        return Ok(());
    }
    // install an engine from the catalog without the UI: chess-rt --install stockfish
    if args.len() >= 3 && args[1] == "--install" {
        let Some(e) = engines::entry(&args[2]) else {
            println!("нет такого движка; есть: {}", engines::CATALOG.iter().map(|e| e.id).collect::<Vec<_>>().join(", "));
            return Ok(());
        };
        let st = engines::download(e.id);
        let mut last = String::new();
        loop {
            let d = st.lock().unwrap().0.clone();
            let line = format!("{d:?}");
            if line != last && !matches!(d, engines::Dl::Downloading { .. }) {
                println!("{line}");
                last = line;
            }
            match d {
                engines::Dl::Done => {
                    println!("готово: {:?} ({})", engines::installed_path(e.id), engines::installed_name(e.id).unwrap_or_default());
                    println!("сила: {:?}", engines::installed_strength(e.id));
                    break;
                }
                engines::Dl::Failed(m) => {
                    println!("ошибка: {m}");
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(200)),
            }
        }
        return Ok(());
    }
    // offline audio demo: chess-rt --wav out.wav [seconds] [night 0/1] [rain 0/1]
    if args.len() >= 3 && args[1] == "--wav" {
        let num = |i: usize, d: f32| args.get(i).and_then(|v| v.parse().ok()).unwrap_or(d);
        let x = audio::render_demo(num(3, 40.0), num(4, 0.0), num(5, 0.0));
        let peak = x.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        audio::write_wav(&args[2], &x).expect("write wav");
        println!("{}: peak {:.3}, rms {:.4}", args[2], peak, rms);
        return Ok(());
    }
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        .require_opengl_es_with_version(3, 0)
        .select()?;

    let app = App::new()?;
    let (tx, rx) = mpsc::channel::<AiResult>();
    let saved_opponent = load_settings(&app);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineReq>();
    {
        let tx = tx.clone();
        std::thread::Builder::new().name("uci".into()).spawn(move || engine_worker(engine_rx, tx)).ok();
    }
    let audio = if std::env::var("CHESS_MUTE").is_ok() {
        None
    } else {
        audio::Audio::start(app.get_music_volume(), app.get_sfx_volume())
    };
    let st = Rc::new(RefCell::new(State {
        audio,
        mode: app.get_mode(),
        opponents: vec![String::new()],
        engine_tx,
        downloads: vec![],
        notice: None,
        game: Game::new(),
        flip: false,
        sel: None,
        sel_moves: vec![],
        hover: None,
        mouse: None,
        drag: None,
        anims: vec![],
        cap: None,
        lift: 0.0,
        pond: Pond::new(),
        day_t: 0.40,
        night_ui: false,
        clock: String::new(),
        promo: None,
        ai_gen: 0,
        ai_busy: false,
        ai_started: 0.0,
        ai_stop: Arc::new(AtomicBool::new(false)),
        ai_tx: tx,
        ai_pending: None,
        start: Instant::now(),
        last_t: 0.0,
        eval: None,
        frames: 0,
        fps_t0: 0.0,
        fps: 0.0,
    }));

    // ---- test hooks: preset moves / selection / mode via env
    if let Ok(moves) = std::env::var("CHESS_MOVES") {
        let mut s = st.borrow_mut();
        for u in moves.split_whitespace() {
            if let Some(m) = s.pos().legal_moves().into_iter().find(|m| m.uci() == u) {
                s.game.play(m);
            } else {
                eprintln!("CHESS_MOVES: illegal {u}");
            }
        }
    }
    if let Ok(m) = std::env::var("CHESS_MODE") {
        app.set_mode(m.parse().unwrap_or(0));
    }
    if let Ok(sel) = std::env::var("CHESS_SELECT") {
        let b = sel.as_bytes();
        if b.len() == 2 {
            let s = (b[1] - b'1') * 8 + (b[0] - b'a');
            st.borrow_mut().select(Some(s));
            st.borrow_mut().lift = 0.10;
        }
    }
    if let Ok(q) = std::env::var("CHESS_Q") {
        app.set_opt_quality(q.parse().unwrap_or(0));
    }
    if std::env::var("CHESS_NOSHADOW").is_ok() {
        app.set_opt_shadows(false);
    }
    if std::env::var("CHESS_ENGINES").is_ok() {
        app.set_engines_open(true);
        refresh_engine_rows(&app, &st.borrow());
    }
    if let Ok(l) = std::env::var("CHESS_LEVEL") {
        app.set_level(l.parse().unwrap_or(12));
    }
    if std::env::var("CHESS_NIGHT").is_ok() {
        app.global::<Theme>().set_night(true);
        st.borrow_mut().pond.night = 1.0;
    }
    if std::env::var("CHESS_RAIN").is_ok() {
        app.set_opt_rain(true);
        st.borrow_mut().pond.rain = 1.0;
    }
    if let Ok(f) = std::env::var("CHESS_FOOD") {
        let v: Vec<f32> = f.split(',').filter_map(|x| x.parse().ok()).collect();
        if v.len() == 2 {
            st.borrow_mut().pond.drop_food(v[0], v[1], 0.0);
        }
    }
    if std::env::var("CHESS_FLIP").is_ok() {
        st.borrow_mut().flip = true;
    }
    if let Ok(sz) = std::env::var("CHESS_SIZE") {
        if let Some((w, h)) = sz.split_once('x') {
            app.window().set_size(slint::PhysicalSize::new(w.parse().unwrap_or(1380), h.parse().unwrap_or(900)));
        }
    }
    let shot_path = std::env::var("CHESS_SHOT").ok();
    let shot_delay: f64 = std::env::var("CHESS_SHOT_DELAY").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let started = Instant::now();
    let mut shot_frame = 0u32;
    let bench_only = std::env::var("CHESS_BENCH").is_ok();
    let bench = bench_only || shot_path.is_some(); // redraw every tick

    {
        let mut s = st.borrow_mut();
        s.opponents = vec![saved_opponent];
        app.set_opponent(0);
        refresh_opponents(&app, &mut s);
        if let Ok(id) = std::env::var("CHESS_OPP") {
            if let Some(i) = s.opponents.iter().position(|o| *o == id) {
                app.set_opponent(i as i32);
            }
        }
        s.flip = app.get_mode() == 1;
        let night = app.global::<Theme>().get_night();
        s.pond.night = night as i32 as f32;
        s.pond.rain = app.get_opt_rain() as i32 as f32;
        s.night_ui = night;
        if night {
            s.day_t = 0.95;
        }
        if let Ok(v) = std::env::var("CHESS_TOD") {
            // test hook: start the day cycle at this time of day (0..1, 0.75 = sunset)
            app.set_opt_daycycle(true);
            s.day_t = v.parse().unwrap_or(0.4);
            let tod = s.day_t as f32;
            let sn = (std::f32::consts::TAU * (tod - 0.25)).sin();
            s.pond.night = ((0.06 - sn) / 0.18).clamp(0.0, 1.0);
            s.night_ui = s.pond.night > 0.5;
            app.global::<Theme>().set_night(s.night_ui);
        }
    }
    sync_ui(&app, &st.borrow());

    // ---- rendering underlay
    {
        let st = st.clone();
        let weak = app.as_weak();
        let mut renderer: Option<Renderer> = None;
        let mut bench_t = Vec::<f64>::new();
        app.window()
            .set_rendering_notifier(move |state, api| match state {
                RenderingState::RenderingSetup => {
                    if let GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                        let gl = unsafe { glow::Context::from_loader_function_cstr(|s| get_proc_address(s)) };
                        match Renderer::new(gl) {
                            Ok(r) => {
                                eprintln!("GL: {}", r.info);
                                renderer = Some(r)
                            }
                            Err(e) => {
                                eprintln!("{e}");
                                std::process::exit(1);
                            }
                        }
                    }
                }
                RenderingState::BeforeRendering => {
                    let (Some(r), Some(app)) = (renderer.as_mut(), weak.upgrade()) else { return };
                    let size = app.window().size();
                    let u = st.borrow_mut().uniforms(&app);
                    let t0 = Instant::now();
                    r.render(size.width, size.height, &u);
                    if bench_only {
                        r.finish();
                        bench_t.push(t0.elapsed().as_secs_f64() * 1000.0);
                        if bench_t.len() == 150 {
                            let mut v = bench_t[30..].to_vec();
                            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            eprintln!(
                                "bench {}x{} hdr={} scale={}: median {:.2} ms, p90 {:.2} ms",
                                size.width,
                                size.height,
                                r.hdr(),
                                u.scale,
                                v[v.len() / 2],
                                v[v.len() * 9 / 10]
                            );
                            let _ = slint::quit_event_loop();
                        }
                    }
                    let s = st.borrow();
                    let gpu = r.info.split(" · ").next().unwrap_or("").replace("Mesa ", "").replace("(R)", "");
                    app.set_fps_text(format!("{}×{} · {:.0} к/с · {}", size.width, size.height, s.fps, gpu).into());
                }
                RenderingState::AfterRendering => {
                    if let (Some(path), Some(r), Some(app)) = (&shot_path, renderer.as_ref(), weak.upgrade()) {
                        if started.elapsed().as_secs_f64() >= shot_delay {
                            shot_frame += 1;
                        }
                        if shot_frame == 6 {
                            let size = app.window().size();
                            let px = r.read_pixels(size.width, size.height);
                            let f = std::fs::File::create(path).unwrap();
                            let mut enc = png::Encoder::new(std::io::BufWriter::new(f), size.width, size.height);
                            enc.set_color(png::ColorType::Rgba);
                            enc.set_depth(png::BitDepth::Eight);
                            enc.write_header().unwrap().write_image_data(&px).unwrap();
                            eprintln!("saved {path}");
                            let _ = slint::quit_event_loop();
                        }
                    }
                }
                RenderingState::RenderingTeardown => {
                    renderer = None;
                }
                _ => {}
            })
            .expect("this needs the OpenGL (femtovg) renderer");
    }

    // ---- input
    {
        let st = st.clone();
        let weak = app.as_weak();
        app.on_pointer(move |kind, x, y, button| {
            let Some(app) = weak.upgrade() else { return };
            let mode = app.get_mode();
            let sq_px = app.get_bsize() / 8.0;
            let b = ((x - app.get_bx()) / sq_px, 8.0 - (y - app.get_by()) / sq_px);
            let mut s = st.borrow_mut();
            match kind {
                0 if button == 1 => {
                    s.select(None);
                    s.drag = None;
                }
                0 => {
                    let Some(sq) = s.square_at(b) else {
                        s.select(None);
                        if (b.0 - 4.0).abs().max((b.1 - 4.0).abs()) > 4.66 {
                            let t = s.now() as f32;
                            s.pond.drop_food(b.0, b.1, t);
                            s.sfx(audio::Sfx::Food, 0.8, b.0, 0.0);
                        }
                        sync_ui(&app, &s);
                        app.window().request_redraw();
                        return;
                    };
                    if !s.human_can_move(mode) || s.animating() {
                        return;
                    }
                    if let Some(m) = s.sel_moves.iter().find(|m| m.to == sq).copied() {
                        if m.promo != 0 {
                            s.promo = Some((m.from, m.to));
                        } else {
                            s.apply_move(m, None);
                        }
                    } else {
                        let pc = s.pos().at(sq);
                        if pc != 0 && color(pc) == s.pos().side && s.sel != Some(sq) {
                            s.select(Some(sq));
                            s.drag = Some(Drag { from: sq, press: b, pos: b, active: false });
                        } else if s.sel == Some(sq) {
                            s.drag = Some(Drag { from: sq, press: b, pos: b, active: false });
                        } else {
                            s.select(None);
                        }
                    }
                }
                1 => {
                    if let Some(d) = s.drag.take() {
                        if d.active {
                            let target = s.square_at(b);
                            match s.sel_moves.iter().find(|m| Some(m.to) == target).copied() {
                                Some(m) if m.promo != 0 => {
                                    s.promo = Some((m.from, m.to));
                                }
                                Some(m) => s.apply_move(m, Some(b)),
                                None => {
                                    // snap back
                                    let now = s.now();
                                    let pc = s.pos().at(d.from);
                                    let to = s.center(d.from);
                                    s.anims.push(Anim { piece: pc, from: b, to, t0: now, dur: 0.14, hide: d.from, lift0: 0.3 });
                                    s.sfx(audio::Sfx::Snap, 0.6, to.0, 0.12);
                                    s.lift = 0.10;
                                }
                            }
                        }
                    }
                }
                2 => {
                    s.mouse = Some(b);
                    s.hover = s.square_at(b);
                    if let Some(d) = s.drag.as_mut() {
                        d.pos = b;
                        if !d.active && ((b.0 - d.press.0).powi(2) + (b.1 - d.press.1).powi(2)).sqrt() > 0.12 {
                            d.active = true;
                        }
                    }
                }
                _ => {
                    s.mouse = None;
                    s.hover = None;
                }
            }
            sync_ui(&app, &s);
            app.window().request_redraw();
        });
    }
    {
        let st = st.clone();
        let weak = app.as_weak();
        app.on_promote(move |k| {
            let Some(app) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            if let Some((f, t)) = s.promo {
                let m = s.sel_moves.iter().find(|m| m.from == f && m.to == t && m.promo == k as u8).copied();
                s.promo = None;
                if let Some(m) = m {
                    s.apply_move(m, None);
                }
            }
            sync_ui(&app, &s);
            app.window().request_redraw();
        });
    }
    {
        let st = st.clone();
        let weak = app.as_weak();
        app.on_new_game(move || {
            let Some(app) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            s.cancel_ai();
            s.notice = None;
            s.mode = app.get_mode();
            s.game = Game::new();
            s.select(None);
            s.anims.clear();
            s.cap = None;
            s.promo = None;
            s.eval = None;
            s.flip = app.get_mode() == 1;
            sync_ui(&app, &s);
            app.window().request_redraw();
        });
    }
    {
        let st = st.clone();
        let weak = app.as_weak();
        app.on_undo(move || {
            let Some(app) = weak.upgrade() else { return };
            let mode = app.get_mode();
            let mut s = st.borrow_mut();
            s.cancel_ai();
            s.select(None);
            s.anims.clear();
            s.cap = None;
            s.promo = None;
            s.game.undo();
            if mode < 2 {
                while s.is_ai_turn(mode) && s.game.undo() {}
            }
            sync_ui(&app, &s);
            app.window().request_redraw();
        });
    }
    {
        let st = st.clone();
        let weak = app.as_weak();
        app.on_flip(move || {
            let Some(app) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            s.flip = !s.flip;
            sync_ui(&app, &s);
            app.window().request_redraw();
        });
    }
    {
        let st = st.clone();
        let weak = app.as_weak();
        app.on_mode_changed(move || {
            let Some(app) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            s.cancel_ai();
            s.select(None);
            s.notice = None;
            s.mode = app.get_mode();
            let opp = s.opponents.get(app.get_opponent() as usize).cloned().unwrap_or_default();
            save_settings(&app, &opp);
            match app.get_mode() {
                0 => s.flip = false,
                1 => s.flip = true,
                _ => {}
            }
            sync_ui(&app, &s);
            app.window().request_redraw();
        });
    }
    {
        let weak = app.as_weak();
        let st = st.clone();
        app.on_settings_changed(move || {
            if let Some(app) = weak.upgrade() {
                let s = st.borrow();
                let opp = s.opponents.get(app.get_opponent() as usize).cloned().unwrap_or_default();
                save_settings(&app, &opp);
                sync_ui(&app, &s);
                app.window().request_redraw();
            }
        });
    }
    {
        let weak = app.as_weak();
        let st = st.clone();
        app.on_volume_changed(move || {
            if let Some(app) = weak.upgrade() {
                let s = st.borrow();
                if let Some(a) = &s.audio {
                    a.volumes(app.get_music_volume(), app.get_sfx_volume());
                }
                let opp = s.opponents.get(app.get_opponent() as usize).cloned().unwrap_or_default();
                save_settings(&app, &opp);
            }
        });
    }
    {
        let weak = app.as_weak();
        let st = st.clone();
        app.on_engines_shown(move || {
            if let Some(app) = weak.upgrade() {
                refresh_engine_rows(&app, &st.borrow());
            }
        });
    }
    {
        let weak = app.as_weak();
        let st = st.clone();
        app.on_engine_action(move |id| {
            let Some(app) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            let Some(e) = engines::entry(&id) else { return };
            let busy = s.downloads.iter().any(|d| {
                d.0 == e.id
                    && matches!(
                        d.1.lock().unwrap().0,
                        engines::Dl::Looking | engines::Dl::Downloading { .. } | engines::Dl::Unpacking | engines::Dl::Checking
                    )
            });
            if busy {
                if let Some(d) = s.downloads.iter().find(|d| d.0 == e.id) {
                    d.1.lock().unwrap().1 = true; // cancel
                }
            } else if engines::installed_path(e.id).is_some() {
                // switching away first so the worker drops the process
                if s.opponents.get(app.get_opponent() as usize).map(|o| o == e.id).unwrap_or(false) {
                    s.cancel_ai();
                    app.set_opponent(0);
                }
                engines::delete(e.id);
                refresh_opponents(&app, &mut s);
            } else {
                s.downloads.retain(|d| d.0 != e.id);
                let dl = engines::download(e.id);
                s.downloads.push((e.id, dl));
            }
            refresh_engine_rows(&app, &s);
            sync_ui(&app, &s);
        });
    }

    // ---- tick: AI results, animation bookkeeping, redraw requests
    let timer = slint::Timer::default();
    {
        let st = st.clone();
        let weak = app.as_weak();
        let mut tick = 0u64;
        let mut test_play = std::env::var("CHESS_PLAY").ok(); // test hook: animated move after 1 s
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(15), move || {
            let Some(app) = weak.upgrade() else { return };
            let mode = app.get_mode();
            let mut s = st.borrow_mut();
            let now = s.now();
            let mut changed = false;
            if now > 1.0 {
                if let Some(u) = test_play.take() {
                    if let Some(m) = s.pos().legal_moves().into_iter().find(|m| m.uci() == u) {
                        s.apply_move(m, None);
                    }
                }
            }
            // finish animations
            let before = s.anims.len();
            s.anims.retain(|a| now < a.t0 + a.dur);
            if s.anims.len() != before {
                changed = true;
            }
            if let Some(mut c) = s.cap {
                if !c.splashed && now > c.t0 + FLIGHT {
                    c.splashed = true;
                    let (x, y) = pond::slot_centre(c.side, c.index);
                    s.pond.ripple(x, y, now as f32, 0.03);
                    s.sfx(audio::Sfx::Splash, 1.0, x, 0.0);
                    s.cap = Some(c);
                    changed = true;
                } else if now > c.t0 + FLIGHT + SINK {
                    s.cap = None;
                }
            }
            while let Ok(r) = rx.try_recv() {
                if r.gen == s.ai_gen {
                    s.ai_pending = Some(r);
                }
            }
            // apply AI move after a short minimum "thinking" time and once animations settle
            if !s.animating() && s.ai_pending.is_some() && now - s.ai_started > 0.35 {
                let r = s.ai_pending.take().unwrap();
                s.ai_busy = false;
                if let Some(err) = &r.err {
                    // the engine failed: tell the user and fall back to the built-in AI
                    s.notice = Some(format!("движок: {err} — играет встроенный ИИ"));
                    app.set_opponent(0);
                }
                s.eval = Some((if r.side == WHITE { r.score } else { -r.score }, r.depth));
                let mv = r.mv.or_else(|| r.uci.as_ref().and_then(|u| s.pos().legal_moves().into_iter().find(|m| &m.uci() == u)));
                if let Some(m) = mv {
                    if s.pos().legal_moves().contains(&m) {
                        s.apply_move(m, None);
                    }
                }
                changed = true;
            }
            if !s.animating() && !s.ai_busy && s.promo.is_none() && s.is_ai_turn(mode) && s.outcome().is_none() {
                s.start_ai(app.get_level(), app.get_opponent() as usize);
                changed = true;
            }
            if changed {
                sync_ui(&app, &s);
            }
            tick += 1;
            // downloads in progress
            if !s.downloads.is_empty() && tick % 8 == 0 {
                let finished: Vec<&'static str> = s
                    .downloads
                    .iter()
                    .filter(|d| matches!(d.1.lock().unwrap().0, engines::Dl::Done))
                    .map(|d| d.0)
                    .collect();
                if !finished.is_empty() {
                    s.downloads.retain(|d| !finished.contains(&d.0));
                    refresh_opponents(&app, &mut s);
                    changed = true;
                }
                refresh_engine_rows(&app, &s);
            }
            // pond sounds
            let evs: Vec<pond::PondEvent> = s.pond.events.drain(..).collect();
            for ev in evs {
                match ev {
                    pond::PondEvent::Eaten(x) => s.sfx(audio::Sfx::Gulp, 0.7, x, 0.0),
                    pond::PondEvent::Lightning => {
                        let d = 0.5 + fastrand::f32() * 1.3;
                        s.sfx(audio::Sfx::Thunder, 1.0, 4.0, d)
                    }
                }
            }
            if app.get_clock_text().as_str() != s.clock {
                app.set_clock_text(s.clock.clone().into());
            }
            if tick % 20 == 0 {
                if let Some(a) = &s.audio {
                    a.mood(s.pond.night, s.pond.rain);
                }
            }
            // the pond is always alive: ~33 fps for it, full rate while something moves on the board
            let ambient = tick % 2 == 0;
            if s.needs_frames(&app) || changed || bench || ambient {
                app.window().request_redraw();
            }
        });
    }

    app.run()
}
