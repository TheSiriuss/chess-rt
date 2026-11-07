//! Life around the board: koi, ripples, food, floating lanterns, fireflies, lightning.
//! Everything lives in board units (squares), y up; the raft spans [-0.62, 8.62]².

use std::f32::consts::PI;

const RAFT: f32 = 4.0 + 0.62; // half size of the raft around the board centre (4, 4)
pub const MAX_RIPPLES: usize = 10;
pub const MAX_FOOD: usize = 6;

pub struct Fish {
    pub x: f32,
    pub y: f32,
    pub a: f32,
    pub speed: f32,
    pub phase: f32,
    pub kind: f32,
    pub size: f32,
    /// swimming depth below the surface
    pub depth: f32,
    /// smoothed turning rate (rad/s): bends the body
    pub turn: f32,
    tx: f32,
    ty: f32,
    retarget: f32,
}

#[derive(Clone, Copy)]
pub struct Ripple {
    pub x: f32,
    pub y: f32,
    pub t0: f32,
    pub amp: f32,
}

#[derive(Clone, Copy)]
pub struct Food {
    pub x: f32,
    pub y: f32,
    pub t0: f32,
}

pub struct Lantern {
    pub x: f32,
    pub y: f32,
    vx: f32,
    vy: f32,
    seed: f32,
}

pub struct Firefly {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    vx: f32,
    vy: f32,
    vz: f32,
    seed: f32,
}

pub enum PondEvent {
    Eaten(f32),
    Lightning,
}

pub struct Pond {
    pub events: Vec<PondEvent>,
    pub fish: Vec<Fish>,
    pub ripples: Vec<Ripple>,
    pub food: Vec<Food>,
    pub lanterns: Vec<Lantern>,
    pub fireflies: Vec<Firefly>,
    pub night: f32,
    pub rain: f32,
    pub flash: f32,
    flash_t0: f32,
    next_flash: f32,
    /// visible water area: x0, x1, y0, y1
    pub bounds: [f32; 4],
}

fn rnd(a: f32, b: f32) -> f32 {
    a + fastrand::f32() * (b - a)
}

fn wrap_angle(mut a: f32) -> f32 {
    while a > PI {
        a -= 2.0 * PI;
    }
    while a < -PI {
        a += 2.0 * PI;
    }
    a
}

fn in_raft(x: f32, y: f32, margin: f32) -> bool {
    (x - 4.0).abs() < RAFT + margin && (y - 4.0).abs() < RAFT + margin
}

impl Pond {
    pub fn new() -> Pond {
        let bounds = [-3.0, 11.0, -1.5, 9.5];
        let mut p = Pond {
            events: vec![],
            fish: vec![],
            ripples: vec![],
            food: vec![],
            lanterns: vec![],
            fireflies: vec![],
            night: 0.0,
            rain: 0.0,
            flash: 0.0,
            flash_t0: -10.0,
            next_flash: 4.0,
            bounds,
        };
        for (i, kind) in [0.0, 1.0, 2.0, 3.0, 4.0, 5.0].into_iter().enumerate() {
            let (x, y) = p.random_water_point();
            p.fish.push(Fish {
                x,
                y,
                a: rnd(-PI, PI),
                speed: 0.3,
                phase: i as f32 * 1.7,
                kind,
                size: rnd(1.2, 1.5),
                depth: rnd(0.10, 0.30),
                turn: 0.0,
                tx: x,
                ty: y,
                retarget: 0.0,
            });
        }
        // lanterns start beside the raft, one on each visible side
        for (i, (x, y)) in [(-1.15, 2.6), (9.2, 5.4), (-1.3, 6.9)].into_iter().enumerate() {
            p.lanterns.push(Lantern { x, y, vx: 0.0, vy: 0.0, seed: i as f32 * 2.3 + 0.7 });
        }
        for i in 0..5 {
            p.fireflies.push(Firefly {
                x: rnd(0.0, 8.0),
                y: rnd(0.0, 8.0),
                z: rnd(0.4, 1.0),
                vx: 0.0,
                vy: 0.0,
                vz: 0.0,
                seed: i as f32 * 1.37 + 0.3,
            });
        }
        p
    }

    /// Random point on visible water, outside the raft.
    fn random_water_point(&self) -> (f32, f32) {
        let [x0, x1, y0, y1] = self.bounds;
        for _ in 0..40 {
            let (x, y) = (rnd(x0, x1), rnd(y0, y1));
            if !in_raft(x, y, 0.35) {
                return (x, y);
            }
        }
        (x0 + 0.5, (y0 + y1) * 0.5)
    }

    pub fn set_bounds(&mut self, b: [f32; 4]) {
        self.bounds = b;
    }

    pub fn ripple(&mut self, x: f32, y: f32, t: f32, amp: f32) {
        self.ripples.retain(|r| t - r.t0 < 6.0);
        if self.ripples.len() >= MAX_RIPPLES {
            self.ripples.remove(0);
        }
        self.ripples.push(Ripple { x, y, t0: t, amp });
    }

    pub fn drop_food(&mut self, x: f32, y: f32, t: f32) {
        if self.food.len() >= MAX_FOOD {
            self.food.remove(0);
        }
        self.food.push(Food { x, y, t0: t });
        self.ripple(x, y, t, 0.010);
    }

    /// `night_target`: 0 day .. 1 night (the day cycle passes fractions)
    pub fn update(&mut self, t: f32, dt: f32, night_target: f32, rain_on: bool) {
        let k = 1.0 - (-dt * 0.9).exp();
        self.night += (night_target.clamp(0.0, 1.0) - self.night) * k;
        self.rain += ((rain_on as i32 as f32) - self.rain) * k;
        let [x0, x1, y0, y1] = self.bounds;

        // ---- koi
        self.food.retain(|f| t - f.t0 < 30.0);
        let mut eaten: Vec<usize> = vec![];
        let mut new_ripples = vec![];
        let n = self.fish.len();
        for i in 0..n {
            // nearest food within reach?
            let (fx, fy) = (self.fish[i].x, self.fish[i].y);
            let food = self
                .food
                .iter()
                .enumerate()
                .filter(|(j, _)| !eaten.contains(j))
                .map(|(j, f)| (j, (f.x - fx).hypot(f.y - fy)))
                .filter(|&(_, d)| d < 6.0)
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            let f = &mut self.fish[i];
            let (target_speed, turn) = if let Some((j, d)) = food {
                f.tx = self.food[j].x;
                f.ty = self.food[j].y;
                if d < 0.16 * f.size {
                    eaten.push(j);
                    new_ripples.push((self.food[j].x, self.food[j].y));
                }
                (0.95, 3.2)
            } else {
                f.retarget -= dt;
                if f.retarget <= 0.0 || (f.tx - f.x).hypot(f.ty - f.y) < 0.5 {
                    f.retarget = rnd(4.0, 9.0);
                    let mut best = (rnd(x0, x1), rnd(y0, y1));
                    for _ in 0..4 {
                        if !in_raft(best.0, best.1, 0.3) {
                            break;
                        }
                        best = (rnd(x0, x1), rnd(y0, y1));
                    }
                    f.tx = best.0;
                    f.ty = best.1;
                }
                (0.32 + 0.1 * (t * 0.3 + i as f32).sin(), 1.4)
            };
            let want = (f.ty - f.y).atan2(f.tx - f.x);
            let d = wrap_angle(want - f.a);
            let da = d.clamp(-turn * dt, turn * dt);
            f.a = wrap_angle(f.a + da);
            let rate = if dt > 0.0 { da / dt } else { 0.0 };
            f.turn += (rate - f.turn) * (1.0 - (-dt * 4.0).exp());
            f.speed += (target_speed - f.speed) * (1.0 - (-dt * 2.0).exp());
            f.x += f.a.cos() * f.speed * dt;
            f.y += f.a.sin() * f.speed * dt;
            f.phase += dt * (3.0 + f.speed * 9.0);
            // don't drift away from the visible pond
            f.x = f.x.clamp(x0 - 1.0, x1 + 1.0);
            f.y = f.y.clamp(y0 - 1.0, y1 + 1.0);
        }
        // gently keep koi apart
        for i in 0..n {
            for j in i + 1..n {
                let (dx, dy) = (self.fish[j].x - self.fish[i].x, self.fish[j].y - self.fish[i].y);
                let d = dx.hypot(dy);
                if d < 0.45 && d > 1e-4 {
                    let push = (0.45 - d) * 0.5 * dt * 4.0;
                    self.fish[i].x -= dx / d * push;
                    self.fish[i].y -= dy / d * push;
                    self.fish[j].x += dx / d * push;
                    self.fish[j].y += dy / d * push;
                }
            }
        }
        eaten.sort_unstable();
        for j in eaten.into_iter().rev() {
            self.food.remove(j);
        }
        for (x, y) in new_ripples {
            self.ripple(x, y, t, 0.014);
            self.events.push(PondEvent::Eaten(x));
        }

        // ---- lanterns drift on the water and stay off the raft
        for l in &mut self.lanterns {
            l.vx += rnd(-1.0, 1.0) * 0.05 * dt;
            l.vy += rnd(-1.0, 1.0) * 0.05 * dt;
            let sp = l.vx.hypot(l.vy);
            if sp > 0.12 {
                l.vx *= 0.12 / sp;
                l.vy *= 0.12 / sp;
            }
            l.x += l.vx * dt;
            l.y += l.vy * dt;
            if l.x < x0 + 0.3 || l.x > x1 - 0.3 {
                l.vx = -l.vx;
                l.x = l.x.clamp(x0 + 0.3, x1 - 0.3);
            }
            if l.y < y0 + 0.3 || l.y > y1 - 0.3 {
                l.vy = -l.vy;
                l.y = l.y.clamp(y0 + 0.3, y1 - 0.3);
            }
            // stay close to the raft so their light reaches the board
            let (dx, dy) = (l.x - 4.0, l.y - 4.0);
            let cheb = dx.abs().max(dy.abs());
            if cheb > RAFT + 1.4 {
                l.vx -= dx / cheb * 0.08 * dt;
                l.vy -= dy / cheb * 0.08 * dt;
            }
            if in_raft(l.x, l.y, 0.3) {
                // push out along the nearest side
                let (dx, dy) = (l.x - 4.0, l.y - 4.0);
                if dx.abs() > dy.abs() {
                    l.x = 4.0 + dx.signum() * (RAFT + 0.3);
                    l.vx = dx.signum() * l.vx.abs();
                } else {
                    l.y = 4.0 + dy.signum() * (RAFT + 0.3);
                    l.vy = dy.signum() * l.vy.abs();
                }
            }
        }

        // ---- fireflies wander in 3D over everything
        for f in &mut self.fireflies {
            f.vx += rnd(-1.0, 1.0) * 0.6 * dt;
            f.vy += rnd(-1.0, 1.0) * 0.6 * dt;
            f.vz += rnd(-1.0, 1.0) * 0.3 * dt;
            let sp = f.vx.hypot(f.vy);
            if sp > 0.35 {
                f.vx *= 0.35 / sp;
                f.vy *= 0.35 / sp;
            }
            f.vz = f.vz.clamp(-0.15, 0.15);
            f.x += f.vx * dt;
            f.y += f.vy * dt;
            f.z += f.vz * dt;
            if f.x < x0 || f.x > x1 {
                f.vx = -f.vx;
                f.x = f.x.clamp(x0, x1);
            }
            if f.y < y0 || f.y > y1 {
                f.vy = -f.vy;
                f.y = f.y.clamp(y0, y1);
            }
            if f.z < 0.35 || f.z > 1.1 {
                f.vz = -f.vz;
                f.z = f.z.clamp(0.35, 1.1);
            }
        }

        // ---- lightning during night storms
        if self.night > 0.6 && self.rain > 0.6 {
            if t > self.next_flash {
                self.flash_t0 = t;
                self.next_flash = t + rnd(5.0, 14.0);
                self.events.push(PondEvent::Lightning);
            }
        } else {
            self.next_flash = self.next_flash.max(t + 3.0);
        }
        let a = t - self.flash_t0;
        self.flash = if (0.0..1.2).contains(&a) {
            let f1 = (-a * 9.0).exp();
            let f2 = if a > 0.16 { 0.8 * (-(a - 0.16) * 7.0).exp() } else { 0.0 };
            let f3 = if a > 0.45 { 0.4 * (-(a - 0.45) * 10.0).exp() } else { 0.0 };
            (f1 + f2 + f3).min(1.0)
        } else {
            0.0
        };
    }

    /// Point lights for the shader: lanterns first, then fireflies. (positions, colours+range, n, n_lanterns)
    pub fn lights(&self, t: f32) -> ([[f32; 4]; 8], [[f32; 4]; 8], i32, i32) {
        let mut pl = [[0.0; 4]; 8];
        let mut col = [[0.0; 4]; 8];
        if self.night < 0.02 {
            return (pl, col, 0, 0);
        }
        let mut n = 0;
        for l in &self.lanterns {
            let flick = 0.85 + 0.08 * (t * 7.1 + l.seed).sin() + 0.05 * (t * 12.7 + l.seed * 3.0).sin();
            pl[n] = [l.x, l.y, 0.45, 3.6 * flick * self.night];
            col[n] = [1.0, 0.55, 0.20, 5.0];
            n += 1;
        }
        let nl = n as i32;
        for f in &self.fireflies {
            let blink = ((t * 1.3 + f.seed * 6.28).sin() * 0.5 + 0.5).powf(3.0);
            pl[n] = [f.x, f.y, f.z, 1.7 * blink * self.night];
            col[n] = [0.70, 1.0, 0.35, 1.7];
            n += 1;
        }
        (pl, col, n as i32, nl)
    }

    pub fn fish_uniforms(&self) -> ([[f32; 4]; 6], [[f32; 4]; 6], i32) {
        let mut a = [[0.0; 4]; 6];
        let mut k = [[0.0; 4]; 6];
        for (i, f) in self.fish.iter().take(6).enumerate() {
            a[i] = [f.x, f.y, f.a, f.phase];
            // turning left curves the tail out to the right
            k[i] = [f.kind, f.size, (-f.turn * 0.32).clamp(-0.55, 0.55), f.depth];
        }
        (a, k, self.fish.len().min(6) as i32)
    }

    pub fn ripple_uniforms(&self) -> [[f32; 4]; MAX_RIPPLES] {
        let mut r = [[0.0; 4]; MAX_RIPPLES];
        for (i, q) in self.ripples.iter().take(MAX_RIPPLES).enumerate() {
            r[i] = [q.x, q.y, q.t0, q.amp];
        }
        r
    }

    pub fn food_uniforms(&self, t: f32) -> [[f32; 4]; MAX_FOOD] {
        let mut r = [[0.0; 4]; MAX_FOOD];
        for (i, f) in self.food.iter().take(MAX_FOOD).enumerate() {
            let bob = 0.01 * (t * 2.0 + i as f32).sin();
            r[i] = [f.x + bob, f.y, 1.0, f.t0];
        }
        r
    }
}

/// Where the n-th captured piece of a side lies on the pond bed (must match the shader).
pub fn slot_centre(side: usize, i: usize) -> (f32, f32) {
    let col = (i / 8) as f32;
    let row = (i % 8) as f32;
    let x = if side == 0 { -1.35 - col * 0.72 } else { 9.35 + col * 0.72 };
    (x, 7.45 - row * 0.98)
}
