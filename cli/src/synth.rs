//! Generador de ráfagas sintéticas con verdad conocida: temblor de mano (desplazamientos
//! sub-píxel), un objeto en movimiento y ruido Poisson-Gauss dependiente de la intensidad.

use camara_core::image::Plane;
use camara_core::package::Frame;

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn uniform(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.uniform()
    }
    pub fn gauss(&mut self) -> f32 {
        let u1 = self.uniform().max(1e-7);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }
}

pub struct Scene {
    pub w: usize,
    pub h: usize,
    circles: Vec<(f32, f32, f32, f32)>,
}

impl Scene {
    pub fn new(w: usize, h: usize, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let mut circles = Vec::new();
        for _ in 0..14 {
            circles.push((
                rng.range(0.05, 0.95) * w as f32,
                rng.range(0.05, 0.55) * h as f32,
                rng.range(8.0, 40.0),
                rng.range(20.0, 235.0),
            ));
        }
        Scene { w, h, circles }
    }

    /// Valor continuo de la escena en (x, y) para la foto `t` (el disco se mueve con t).
    fn eval(&self, x: f32, y: f32, t: usize) -> f32 {
        let (wf, hf) = (self.w as f32, self.h as f32);
        let mut v = 70.0 + 90.0 * (x / wf) + 15.0 * (y * 0.02).sin();
        for &(cx, cy, r, val) in &self.circles {
            let (dx, dy) = (x - cx, y - cy);
            if dx * dx + dy * dy < r * r {
                v = val;
            }
        }
        // Rejilla de alta frecuencia (período ≈ 6 px): prueba de recuperación de detalle.
        if x > wf * 0.05 && x < wf * 0.30 && y > hf * 0.62 && y < hf * 0.95 {
            v = 128.0 + 70.0 * (x * 0.9 + y * 0.4).sin();
        }
        // Estrella de radios.
        let (sx, sy) = (wf * 0.62, hf * 0.78);
        let (dx, dy) = (x - sx, y - sy);
        if dx * dx + dy * dy < (hf * 0.17).powi(2) {
            v = if (dy.atan2(dx) * 10.0).sin() > 0.0 { 215.0 } else { 40.0 };
        }
        // Barras tipo texto.
        if x > wf * 0.35 && x < wf * 0.55 && y > hf * 0.62 && y < hf * 0.9 {
            v = if ((y - hf * 0.62) / 4.0) as i32 % 2 == 0 { 30.0 } else { 225.0 };
        }
        // Objeto en movimiento.
        let (mx, my) = (wf * 0.45 + t as f32 * 7.0, hf * 0.30 + t as f32 * 2.0);
        let (dx, dy) = (x - mx, y - my);
        if dx * dx + dy * dy < 28.0 * 28.0 {
            v = 250.0;
        }
        v
    }

    /// Renderiza sin ruido con desplazamiento (sx, sy) y supermuestreo 3x3.
    pub fn render(&self, sx: f32, sy: f32, t: usize) -> Plane {
        let mut p = Plane::new(self.w, self.h);
        for y in 0..self.h {
            for x in 0..self.w {
                let mut s = 0.0;
                for j in 0..3 {
                    for i in 0..3 {
                        s += self.eval(x as f32 + sx + (i as f32 + 0.5) / 3.0 - 0.5, y as f32 + sy + (j as f32 + 0.5) / 3.0 - 0.5, t);
                    }
                }
                p.data[y * self.w + x] = s / 9.0;
            }
        }
        p
    }
}

pub struct Burst {
    pub frames: Vec<Frame>,
    pub shifts: Vec<(f32, f32)>,
}

/// Ruido: var = a·v + b (en escala 0..255).
pub fn make_burst(scene: &Scene, n: usize, a: f32, b: f32, seed: u64) -> Burst {
    let mut rng = Rng::new(seed);
    let mut frames = Vec::new();
    let mut shifts = Vec::new();
    for t in 0..n {
        let (sx, sy) = (rng.range(-3.0, 3.0), rng.range(-3.0, 3.0));
        let clean = scene.render(sx, sy, t);
        let luma: Vec<u16> = clean
            .data
            .iter()
            .map(|&v| {
                let sigma = (a * v + b).sqrt();
                (v + sigma * rng.gauss()).round().clamp(0.0, 255.0) as u16
            })
            .collect();
        // Croma sintético ligado a la estructura de la escena (4:2:0) con ruido propio.
        let (cw, ch) = ((scene.w + 1) / 2, (scene.h + 1) / 2);
        let mut cb = vec![0u16; cw * ch];
        let mut cr = vec![0u16; cw * ch];
        for j in 0..ch {
            for i in 0..cw {
                let mut s = 0.0;
                for dy in 0..2 {
                    for dx in 0..2 {
                        s += clean.get((2 * i + dx) as isize, (2 * j + dy) as isize);
                    }
                }
                let v = s / 4.0 - 128.0;
                cb[j * cw + i] = (128.0 + 0.45 * v + 6.0 * rng.gauss()).round().clamp(0.0, 255.0) as u16;
                cr[j * cw + i] = (128.0 - 0.35 * v + 6.0 * rng.gauss()).round().clamp(0.0, 255.0) as u16;
            }
        }
        frames.push(Frame {
            width: scene.w as u32,
            height: scene.h as u32,
            white_level: 255,
            iso: 1600,
            exposure_ns: 33_000_000,
            rotation: 0,
            luma,
            cw: cw as u32,
            ch: ch as u32,
            cb,
            cr,
        });
        shifts.push((sx, sy));
    }
    Burst { frames, shifts }
}
