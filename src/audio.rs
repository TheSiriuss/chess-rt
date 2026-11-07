//! Sound: a small synthesizer streaming to PulseAudio / PipeWire.
//! Generative pond music (koto plucks, pads, bass, night flute, water drips) and soft tonal
//! effects. Everything is built from sines with gentle attacks: no noise, no square waves.

use std::f64::consts::TAU;
use std::sync::mpsc::{channel, Receiver, Sender};

pub const SR: f32 = 44100.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sfx {
    Lift,
    Move,
    Capture,
    Splash,
    Check,
    Win,
    Lose,
    Draw,
    Promote,
    Food,
    Gulp,
    Snap,
    Thunder,
}

pub const ALL_SFX: [Sfx; 13] = [
    Sfx::Lift,
    Sfx::Move,
    Sfx::Capture,
    Sfx::Splash,
    Sfx::Check,
    Sfx::Win,
    Sfx::Lose,
    Sfx::Draw,
    Sfx::Promote,
    Sfx::Food,
    Sfx::Gulp,
    Sfx::Snap,
    Sfx::Thunder,
];

pub enum Cmd {
    /// effect, volume, pan (-1..1), delay in seconds
    Play(Sfx, f32, f32, f32),
    Volumes(f32, f32),
    /// night 0..1, rain 0..1
    Mood(f32, f32),
    Quit,
}

#[derive(Clone)]
pub struct Audio {
    tx: Sender<Cmd>,
}

impl Audio {
    pub fn play_in(&self, s: Sfx, vol: f32, pan: f32, delay: f32) {
        let _ = self.tx.send(Cmd::Play(s, vol, pan, delay));
    }
    pub fn volumes(&self, music: f32, sfx: f32) {
        let _ = self.tx.send(Cmd::Volumes(music, sfx));
    }
    pub fn mood(&self, night: f32, rain: f32) {
        let _ = self.tx.send(Cmd::Mood(night, rain));
    }

    /// Open the default PulseAudio (PipeWire) output and start the mixer thread.
    /// None when there is no sound server: the game then stays silent.
    pub fn start(music: f32, sfx: f32) -> Option<Audio> {
        use libpulse_binding as pulse;
        use libpulse_simple_binding as psimple;
        let spec = pulse::sample::Spec { format: pulse::sample::Format::F32le, channels: 2, rate: SR as u32 };
        if !spec.is_valid() {
            return None;
        }
        let attr = pulse::def::BufferAttr {
            maxlength: u32::MAX,
            tlength: (SR * 0.08) as u32 * 8,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: u32::MAX,
        };
        let simple =
            psimple::Simple::new(None, "Шахматы RT", pulse::stream::Direction::Playback, None, "музыка и звуки", &spec, None, Some(&attr))
                .ok()?;
        let (tx, rx) = channel::<Cmd>();
        let _ = tx.send(Cmd::Volumes(music, sfx));
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let mut synth = Synth::new();
                let mut buf = vec![0f32; 441 * 2];
                let mut bytes: Vec<u8> = Vec::with_capacity(buf.len() * 4);
                loop {
                    if !synth.drain(&rx) {
                        return;
                    }
                    synth.render(&mut buf);
                    bytes.clear();
                    for s in &buf {
                        bytes.extend_from_slice(&s.to_le_bytes());
                    }
                    if simple.write(&bytes).is_err() {
                        return;
                    }
                }
            })
            .ok()?;
        Some(Audio { tx })
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
    }
}

// ---------------------------------------------------------------- helpers
fn atk(t: f32, a: f32) -> f32 {
    if t >= a {
        1.0
    } else {
        let x = t / a;
        x * x * (3.0 - 2.0 * x)
    }
}
fn dec(t: f32, tau: f32) -> f32 {
    (-t / tau).exp()
}
fn s(ph: f64) -> f32 {
    ph.sin() as f32
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x as f32 / u32::MAX as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
    fn pick(&mut self, n: usize) -> usize {
        ((self.next() * n as f32) as usize).min(n - 1)
    }
}

// ---------------------------------------------------------------- instruments
#[derive(Clone, Copy, PartialEq, Debug)]
enum Inst {
    Koto,
    Bass,
    Pad,
    Flute,
    Drip,
    Tok,
    Bell,
    Bloop,
    Rumble,
}

#[derive(Clone, Copy)]
struct Note {
    inst: Inst,
    f: f32,
    vel: f32,
    pan: f32,
    /// sustain length for pads / flute
    hold: f32,
    t: f32,
    ph: f64,
    music: bool,
}

impl Note {
    fn life(&self) -> f32 {
        match self.inst {
            Inst::Koto => 4.5,
            Inst::Bass => 5.0,
            Inst::Pad | Inst::Flute => self.hold + 3.0,
            Inst::Drip => 0.6,
            Inst::Tok => 0.6,
            Inst::Bell => 6.0,
            Inst::Bloop => 0.55,
            Inst::Rumble => 10.0,
        }
    }

    /// next mono sample
    fn sample(&mut self) -> f32 {
        let t = self.t;
        let f = self.f as f64;
        let td = t as f64;
        let dt = 1.0 / SR as f64;
        let v = match self.inst {
            Inst::Koto => {
                let a = atk(t, 0.007);
                a * (s(TAU * f * td) * dec(t, 1.1)
                    + 0.42 * s(TAU * 2.0 * f * td) * dec(t, 0.45)
                    + 0.16 * s(TAU * 3.01 * f * td) * dec(t, 0.22)
                    + 0.07 * s(TAU * 4.2 * f * td) * dec(t, 0.12))
                    * 0.55
            }
            Inst::Bass => atk(t, 0.012) * (s(TAU * f * td) + 0.25 * s(TAU * 2.0 * f * td) * dec(t, 0.4)) * dec(t, 1.6) * 0.8,
            Inst::Pad => {
                let env = atk(t, 1.4) * if t > self.hold { dec(t - self.hold, 0.9) } else { 1.0 };
                let trem = 1.0 + 0.08 * (TAU * 0.23 * td).sin() as f32;
                env * trem
                    * (s(TAU * f * td) + s(TAU * f * 1.0035 * td) + 0.45 * s(TAU * f * 1.997 * td) + 0.18 * s(TAU * f * 3.0 * td))
                    * 0.16
            }
            Inst::Flute => {
                let vib = 1.0 + 0.007 * (TAU * 5.1 * td).sin() * (atk(t, 0.6) as f64) * (t > 0.25) as i32 as f64;
                self.ph += TAU * f * vib * dt;
                let env = atk(t, 0.14) * if t > self.hold { dec(t - self.hold, 0.35) } else { 1.0 };
                env * (s(self.ph) + 0.16 * s(2.0 * self.ph) + 0.05 * s(3.0 * self.ph)) * 0.5
            }
            Inst::Drip => {
                let fi = f * (1.0 + 0.6 * (1.0 - (-td / 0.025).exp()));
                self.ph += TAU * fi * dt;
                atk(t, 0.008) * dec(t, 0.06) * s(self.ph) * 0.5
            }
            Inst::Tok => {
                // wooden knock: a short sine with a slight pitch drop and a damped overtone
                let fi = f * (1.0 + 0.06 * (-td / 0.015).exp());
                self.ph += TAU * fi * dt;
                atk(t, 0.006) * dec(t, 0.075) * (s(self.ph) + 0.3 * s(2.43 * self.ph) * dec(t, 0.03))
            }
            Inst::Bell => {
                atk(t, 0.006)
                    * (s(TAU * f * td) * dec(t, 1.4) + 0.22 * s(TAU * 2.0 * f * td) * dec(t, 0.5) + 0.06 * s(TAU * 2.76 * f * td) * dec(t, 0.18))
                    * 0.6
            }
            Inst::Bloop => {
                // a drop into water: rising pitch, quick decay
                let fi = f * (1.0 + 1.1 * (1.0 - (-td / 0.04).exp()));
                self.ph += TAU * fi * dt;
                atk(t, 0.007) * dec(t, 0.075) * s(self.ph)
            }
            Inst::Rumble => {
                let mut x = 0.0;
                for (k, (fr, m)) in [(38.0, 0.7), (46.0, 1.1), (53.0, 1.7), (61.0, 2.3)].iter().enumerate() {
                    x += s(TAU * fr * td + k as f64) * (0.6 + 0.4 * s(TAU * m * td + k as f64 * 1.7));
                }
                atk(t, 0.35) * dec(t, 2.2) * x * 0.35
            }
        };
        // a guaranteed smooth end
        let left = self.life() - t;
        let fade = if left < 0.08 { (left / 0.08).max(0.0) } else { 1.0 };
        self.t += 1.0 / SR;
        v * self.vel * fade
    }
}

fn note(inst: Inst, f: f32, vel: f32, pan: f32, music: bool) -> Note {
    Note { inst, f, vel, pan, hold: 0.0, t: 0.0, ph: 0.0, music }
}

// ---------------------------------------------------------------- reverb (Freeverb-style)
struct Comb {
    buf: Vec<f32>,
    i: usize,
    z: f32,
}
struct Allpass {
    buf: Vec<f32>,
    i: usize,
}
struct Reverb {
    combs: [Vec<Comb>; 2],
    aps: [Vec<Allpass>; 2],
}
impl Reverb {
    fn new() -> Reverb {
        let mk = |off: usize| {
            (
                [1116, 1188, 1277, 1356, 1422, 1491].iter().map(|&n| Comb { buf: vec![0.0; n + off], i: 0, z: 0.0 }).collect::<Vec<_>>(),
                [556, 441, 341].iter().map(|&n| Allpass { buf: vec![0.0; n + off], i: 0 }).collect::<Vec<_>>(),
            )
        };
        let (cl, al) = mk(0);
        let (cr, ar) = mk(23);
        Reverb { combs: [cl, cr], aps: [al, ar] }
    }
    fn process(&mut self, x: f32, fb: f32, damp: f32) -> (f32, f32) {
        let mut out = [0.0f32; 2];
        for ch in 0..2 {
            let mut y = 0.0;
            for c in &mut self.combs[ch] {
                let o = c.buf[c.i];
                c.z = o * (1.0 - damp) + c.z * damp;
                c.buf[c.i] = x + c.z * fb;
                c.i = (c.i + 1) % c.buf.len();
                y += o;
            }
            for a in &mut self.aps[ch] {
                let b = a.buf[a.i];
                let o = b - y;
                a.buf[a.i] = y + b * 0.5;
                a.i = (a.i + 1) % a.buf.len();
                y = o;
            }
            out[ch] = y * 0.12;
        }
        (out[0], out[1])
    }
}

// ---------------------------------------------------------------- music
const D3: f32 = 146.83;
/// day: yo scale (D E G A B), night: in scale (D Eb G A Bb), in semitones
const YO: [i32; 5] = [0, 2, 7, 9, 11];
const IN: [i32; 5] = [0, 1, 7, 9, 10];
/// chord roots (scale degrees), two bars each
const PROG: [i32; 4] = [0, 3, 2, 4];

fn pitch(scale: &[i32; 5], degree: i32, octave: i32) -> f32 {
    let d = degree.rem_euclid(5);
    let o = octave + degree.div_euclid(5);
    D3 * 2f32.powf((scale[d as usize] + 12 * o) as f32 / 12.0)
}

pub struct Synth {
    notes: Vec<Note>,
    pending: Vec<(u32, Note)>,
    rng: Rng,
    rev: Reverb,
    music_vol: f32,
    sfx_vol: f32,
    night: f32,
    rain: f32,
    // music clock
    step: i64,
    to_step: f32,
    melody: i32,
    flute_left: i32,
    lp: [f32; 2],
    fade_in: f32,
}

impl Synth {
    pub fn new() -> Synth {
        Synth {
            notes: Vec::with_capacity(64),
            pending: vec![],
            rng: Rng(0x9E37_79B9),
            rev: Reverb::new(),
            music_vol: 0.55,
            sfx_vol: 0.8,
            night: 0.0,
            rain: 0.0,
            step: -1,
            to_step: 0.0,
            melody: 7,
            flute_left: 0,
            lp: [0.0; 2],
            fade_in: 0.0,
        }
    }

    /// Apply queued commands; false = quit.
    fn drain(&mut self, rx: &Receiver<Cmd>) -> bool {
        while let Ok(c) = rx.try_recv() {
            match c {
                Cmd::Quit => return false,
                c => self.handle(c),
            }
        }
        true
    }

    pub fn handle(&mut self, c: Cmd) {
        match c {
            Cmd::Play(s, vol, pan, delay) => self.sfx(s, vol, pan, delay),
            Cmd::Volumes(m, s) => {
                self.music_vol = m.clamp(0.0, 1.0);
                self.sfx_vol = s.clamp(0.0, 1.0);
            }
            Cmd::Mood(n, r) => {
                self.night = n.clamp(0.0, 1.0);
                self.rain = r.clamp(0.0, 1.0);
            }
            Cmd::Quit => {}
        }
    }

    fn at(&mut self, delay: f32, n: Note) {
        let d = (delay.max(0.0) * SR) as u32;
        if d == 0 {
            self.notes.push(n);
        } else {
            self.pending.push((d, n));
        }
    }

    fn sfx(&mut self, s: Sfx, vol: f32, pan: f32, delay: f32) {
        let scale = if self.night > 0.5 { &IN } else { &YO };
        let add = |me: &mut Synth, dt: f32, inst: Inst, f: f32, v: f32| {
            me.at(delay + dt, note(inst, f, v * vol, pan, false));
        };
        match s {
            Sfx::Lift => add(self, 0.0, Inst::Tok, 400.0, 0.18),
            Sfx::Move => add(self, 0.0, Inst::Tok, 200.0, 0.55),
            Sfx::Capture => {
                add(self, 0.0, Inst::Tok, 170.0, 0.6);
                add(self, 0.045, Inst::Tok, 250.0, 0.25);
            }
            Sfx::Splash => {
                add(self, 0.0, Inst::Bloop, 190.0, 0.55);
                for k in 0..4 {
                    let f = 420.0 + 160.0 * k as f32 + self.rng.range(-30.0, 30.0);
                    let dt = 0.09 + 0.08 * k as f32 + self.rng.range(0.0, 0.05);
                    add(self, dt, Inst::Bloop, f, 0.18 / (1.0 + k as f32 * 0.4));
                }
            }
            Sfx::Check => {
                // a soft low temple bell, same register as the wooden knocks
                add(self, 0.0, Inst::Bell, pitch(scale, 2, 0), 0.5);
                add(self, 0.14, Inst::Bell, pitch(scale, 4, 0), 0.3);
            }
            Sfx::Win => {
                for (k, d) in [0, 2, 3, 4, 5].iter().enumerate() {
                    add(self, k as f32 * 0.13, Inst::Koto, pitch(&YO, *d, 0), 0.55);
                }
                add(self, 0.65, Inst::Bell, pitch(&YO, 0, 0), 0.35);
            }
            Sfx::Lose => {
                for (k, d) in [7, 5, 3, 1, 0].iter().enumerate() {
                    add(self, k as f32 * 0.2, Inst::Koto, pitch(&IN, *d, 0), 0.5);
                }
            }
            Sfx::Draw => {
                add(self, 0.0, Inst::Koto, pitch(scale, 3, 1), 0.45);
                add(self, 0.22, Inst::Koto, pitch(scale, 0, 1), 0.45);
            }
            Sfx::Promote => {
                for k in 0..5 {
                    add(self, k as f32 * 0.07, Inst::Koto, pitch(scale, k, 0), 0.45);
                }
            }
            Sfx::Food => add(self, 0.0, Inst::Bloop, 300.0, 0.3),
            Sfx::Gulp => {
                add(self, 0.0, Inst::Bloop, 300.0, 0.22);
                add(self, 0.07, Inst::Bloop, 380.0, 0.15);
            }
            Sfx::Snap => add(self, 0.0, Inst::Tok, 130.0, 0.4),
            Sfx::Thunder => add(self, 0.0, Inst::Rumble, 1.0, 0.9),
        }
    }

    fn music_step(&mut self) {
        self.step += 1;
        let st = self.step;
        let night = self.night > 0.5;
        let scale = if night { IN } else { YO };
        let bar = st / 8;
        let in_bar = st % 8;
        let chord = PROG[((bar / 2) % 4) as usize];
        let r = |me: &mut Synth| me.rng.next();

        // pads: a new chord every two bars
        if st % 16 == 0 {
            let hold = 16.0 * self.step_len() + 0.4;
            for (k, deg) in [chord, chord + 2, chord + 4].iter().enumerate() {
                let mut n = note(Inst::Pad, pitch(&scale, *deg, 1), 0.55, [-0.35, 0.0, 0.35][k], true);
                n.hold = hold;
                self.notes.push(n);
            }
        }
        // bass on the first beat of a bar, and lightly on the third in the day
        if in_bar == 0 || (!night && in_bar == 4 && r(self) < 0.5) {
            let deg = if in_bar == 0 { chord } else { chord + 2 };
            self.notes.push(note(Inst::Bass, pitch(&scale, deg, -1), 0.5, 0.0, true));
        }
        // koto melody: a random walk that leans on chord tones
        let density = if night { 0.16 } else { 0.30 };
        if r(self) < density * if in_bar % 2 == 0 { 1.3 } else { 0.7 } {
            let steps = [-2, -1, -1, 1, 1, 2, 0];
            let mut m = self.melody + steps[self.rng.pick(steps.len())];
            if in_bar == 0 && r(self) < 0.6 {
                // snap to the nearest chord tone
                let cands = [chord + 5, chord + 7, chord + 9, chord + 10, chord + 12];
                m = *cands.iter().min_by_key(|c| (*c - m).abs()).unwrap();
            }
            m = m.clamp(4, 15);
            self.melody = m;
            let pan = self.rng.range(-0.5, 0.5);
            let vel = self.rng.range(0.35, 0.6);
            self.notes.push(note(Inst::Koto, pitch(&scale, m, 0), vel, pan, true));
            // occasional grace note just before
            if r(self) < 0.08 {
                let g = note(Inst::Koto, pitch(&scale, m + 1, 0), vel * 0.5, pan, true);
                self.at(self.step_len() * 0.5, g);
            }
        }
        // night flute phrases
        if night && self.flute_left == 0 && in_bar == 0 && bar % 8 == 4 && r(self) < 0.7 {
            self.flute_left = 3 + self.rng.pick(3) as i32;
        }
        if self.flute_left > 0 && in_bar % 4 == 0 && r(self) < 0.8 {
            self.flute_left -= 1;
            let deg = chord + 5 + [0, 2, 4, 1, 3][self.rng.pick(5)];
            let mut n = note(Inst::Flute, pitch(&scale, deg, 1), 0.28, self.rng.range(-0.3, 0.3), true);
            n.hold = self.step_len() * self.rng.range(3.0, 7.0);
            self.notes.push(n);
        }
        // water drips, more of them in the rain
        let drip = 0.05 + 0.5 * self.rain;
        if r(self) < drip {
            let f = self.rng.range(900.0, 1700.0);
            let pan = self.rng.range(-0.8, 0.8);
            let d = self.rng.range(0.0, self.step_len());
            let v = self.rng.range(0.10, 0.22) * (1.0 - 0.4 * self.rain);
            self.at(d, note(Inst::Drip, f, v, pan, true));
        }
    }

    fn step_len(&self) -> f32 {
        let bpm = 64.0 - 10.0 * self.night;
        60.0 / bpm / 2.0
    }

    pub fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        // scheduled notes whose time has come
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].0 as usize <= frames {
                let (_, n) = self.pending.swap_remove(i);
                self.notes.push(n);
            } else {
                self.pending[i].0 -= frames as u32;
                i += 1;
            }
        }
        let wet_fb = 0.80 + 0.06 * self.night;
        for fr in 0..frames {
            self.to_step -= 1.0 / SR;
            if self.to_step <= 0.0 {
                self.to_step += self.step_len();
                if self.music_vol > 0.001 {
                    self.music_step();
                }
            }
            self.fade_in = (self.fade_in + 1.0 / (SR * 3.0)).min(1.0);
            let (mut l, mut r, mut send) = (0.0f32, 0.0f32, 0.0f32);
            for n in &mut self.notes {
                let v = n.sample();
                let g = if n.music { self.music_vol * self.fade_in } else { self.sfx_vol };
                let x = v * g;
                let p = n.pan.clamp(-1.0, 1.0);
                l += x * (1.0 - p).sqrt() * 0.707;
                r += x * (1.0 + p).sqrt() * 0.707;
                send += x * if n.music { 0.55 + 0.3 * self.night } else { 0.25 };
            }
            let (wl, wr) = self.rev.process(send, wet_fb, 0.35);
            let mut o = [l + wl, r + wr];
            for ch in 0..2 {
                // gentle low-pass (≈ 7 kHz) and soft limiting
                self.lp[ch] += (o[ch] - self.lp[ch]) * 0.63;
                let x = self.lp[ch] * 0.8;
                o[ch] = x * (27.0 + x * x) / (27.0 + 9.0 * x * x);
            }
            out[fr * 2] = o[0];
            out[fr * 2 + 1] = o[1];
        }
        self.notes.retain(|n| n.t < n.life());
    }
}

/// Offline render for tests and `--wav`: `secs` of music with every effect played once.
pub fn render_demo(secs: f32, night: f32, rain: f32) -> Vec<f32> {
    let mut s = Synth::new();
    s.night = night;
    s.rain = rain;
    for (k, e) in ALL_SFX.iter().enumerate() {
        s.sfx(*e, 0.9, 0.0, 2.0 + k as f32 * 1.2);
    }
    let mut out = vec![0.0f32; (secs * SR) as usize * 2];
    for chunk in out.chunks_mut(882) {
        s.render(chunk);
    }
    out
}

pub fn write_wav(path: &str, samples: &[f32]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data = (samples.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&(SR as u32).to_le_bytes())?;
    f.write_all(&(SR as u32 * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data.to_le_bytes())?;
    for s in samples {
        f.write_all(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// one effect alone, mono, sfx bus only, no reverb
    fn solo(e: Sfx) -> Vec<f32> {
        let mut s = Synth::new();
        s.sfx(e, 1.0, 0.0, 0.0);
        let mut out = vec![];
        for _ in 0..(SR as usize * 11) {
            // advance pending
            let mut i = 0;
            while i < s.pending.len() {
                if s.pending[i].0 == 0 {
                    let (_, n) = s.pending.swap_remove(i);
                    s.notes.push(n);
                } else {
                    s.pending[i].0 -= 1;
                    i += 1;
                }
            }
            let v: f32 = s.notes.iter_mut().map(|n| n.sample()).sum();
            s.notes.retain(|n| n.t < n.life());
            out.push(v);
            if s.notes.is_empty() && s.pending.is_empty() {
                break;
            }
        }
        out
    }

    /// Why a sound would be heard as a pop, click, hiss or buzz (None = smooth).
    fn pop_report(x: &[f32]) -> Option<String> {
        let peak = x.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        if peak == 0.0 {
            return Some("silent".into());
        }
        let ip = x.iter().position(|v| v.abs() == peak).unwrap();
        if (ip as f32 / 44.1) < 4.0 {
            return Some(format!("peaks after only {:.1} ms", ip as f32 / 44.1));
        }
        let first = x[..44].iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        if first >= 0.12 * peak {
            return Some(format!("starts with a step: {first} of {peak}"));
        }
        let d: f32 = x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        let e: f32 = x.iter().map(|v| v * v).sum();
        if d / e >= 0.35 {
            return Some(format!("too bright: {:.2}", d / e));
        }
        let tail = x[x.len().saturating_sub(441)..].iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        if tail >= 0.03 * peak {
            return Some(format!("cut off abruptly: {tail} of {peak}"));
        }
        None
    }

    #[test]
    fn no_effect_pops_clicks_or_hisses() {
        for e in ALL_SFX {
            let x = solo(e);
            if let Some(why) = pop_report(&x) {
                panic!("{e:?}: {why}");
            }
        }
    }

    /// "average pitch" of a sound: rms frequency from the derivative energy
    fn rms_freq(x: &[f32]) -> f32 {
        let d: f32 = x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        let e: f32 = x.iter().map(|v| v * v).sum();
        (d / e).sqrt() * SR / std::f32::consts::TAU
    }

    #[test]
    fn effects_stay_in_a_low_soft_register() {
        // the wooden knocks sit at 150-500 Hz; nothing may stick out far above them
        let mut report = vec![];
        for e in ALL_SFX {
            let f = rms_freq(&solo(e));
            report.push(format!("{e:?} {f:.0} Hz"));
            assert!(f < 620.0, "{e:?} is too high: {f:.0} Hz ({})", report.join(", "));
        }
        println!("{}", report.join(", "));
    }

    #[test]
    fn the_pop_detector_catches_a_click() {
        let mut x = vec![0.0f32; 4410];
        for (i, v) in x.iter_mut().enumerate().skip(100) {
            *v = (i as f32 * 0.05).sin() * (-(i as f32) / 2000.0).exp();
        }
        assert!(pop_report(&x).is_some());
    }

    #[test]
    fn demo_is_finite_bounded_and_audible() {
        for (night, rain) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            let x = render_demo(30.0, night, rain);
            assert!(x.iter().all(|v| v.is_finite()));
            let peak = x.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
            let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
            assert!(peak < 1.0, "clipping: {peak}");
            assert!(rms > 0.01, "too quiet: {rms}");
        }
    }

    #[test]
    fn muted_is_silent() {
        let mut s = Synth::new();
        s.music_vol = 0.0;
        s.sfx_vol = 0.0;
        s.sfx(Sfx::Win, 1.0, 0.0, 0.0);
        let mut out = vec![0.0f32; 44100 * 2];
        for c in out.chunks_mut(882) {
            s.render(c);
        }
        assert!(out.iter().all(|v| v.abs() < 1e-6));
    }
}
