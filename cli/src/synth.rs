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
        // Textura fina de bajo contraste (tela, piel, follaje, plástico mate): amplitud ±8 niveles.
        if x > wf * TEXTURE.0 && x < wf * TEXTURE.2 && y > hf * TEXTURE.1 && y < hf * TEXTURE.3 {
            let t = ((1.9 * x + 0.7 * y).sin() + (2.3 * y - 0.4 * x).sin() + (1.1 * x + 1.7 * y).sin() + (2.9 * x - 1.3 * y).sin()) / 4.0;
            v = 120.0 + 8.0 * t * 2.0;
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
        self.render_rot(sx, sy, 0.0, t)
    }

    /// Igual, con una rotación de `rot` grados alrededor del centro (temblor real de la mano).
    pub fn render_rot(&self, sx: f32, sy: f32, rot: f32, t: usize) -> Plane {
        let mut p = Plane::new(self.w, self.h);
        let (cx, cy) = (self.w as f32 / 2.0, self.h as f32 / 2.0);
        let (s, c) = rot.to_radians().sin_cos();
        for y in 0..self.h {
            for x in 0..self.w {
                let mut acc = 0.0;
                for j in 0..3 {
                    for i in 0..3 {
                        let px = x as f32 + (i as f32 + 0.5) / 3.0 - 0.5 - cx;
                        let py = y as f32 + (j as f32 + 0.5) / 3.0 - 0.5 - cy;
                        let (rx, ry) = (c * px - s * py + cx, s * px + c * py + cy);
                        acc += self.eval(rx + sx, ry + sy, t);
                    }
                }
                p.data[y * self.w + x] = acc / 9.0;
            }
        }
        p
    }
}

pub struct Burst {
    pub frames: Vec<Frame>,
    pub shifts: Vec<(f32, f32)>,
    /// Rotación de cada foto en grados (para reconstruir la verdad de la referencia).
    pub rots: Vec<f32>,
}

/// Zona de textura fina de bajo contraste (x0,y0,x1,y1 como fracciones del tamaño).
pub const TEXTURE: (f32, f32, f32, f32) = (0.60, 0.40, 0.92, 0.58);

/// Regiones de la escena HDR (x0,y0,x1,y1 como fracciones del tamaño).
pub const LAMP: (f32, f32, f32, f32) = (0.72, 0.10, 0.94, 0.30);
pub const SHADOW: (f32, f32, f32, f32) = (0.06, 0.12, 0.28, 0.34);

impl Scene {
    /// Radiancia LINEAL de la escena (1.0 = blanco de la exposición de referencia).
    /// Incluye una lámpara muy brillante (>1, se satura en la referencia) y una zona
    /// muy oscura con textura tenue: el reto típico del HDR.
    fn eval_rad(&self, x: f32, y: f32, t: usize) -> f32 {
        let (wf, hf) = (self.w as f32, self.h as f32);
        let v = self.eval(x, y, t);
        let mut rad = (v / 255.0).powf(2.2);
        if x > wf * LAMP.0 && x < wf * LAMP.2 && y > hf * LAMP.1 && y < hf * LAMP.3 {
            rad = 3.0 + 1.2 * (x * 0.35).sin() * (y * 0.35).sin();
        }
        if x > wf * SHADOW.0 && x < wf * SHADOW.2 && y > hf * SHADOW.1 && y < hf * SHADOW.3 {
            rad = 0.010 + 0.006 * (x * 0.30 + y * 0.22).sin();
        }
        rad
    }

    /// Radiancia lineal con supermuestreo 3x3.
    pub fn render_rad(&self, sx: f32, sy: f32, t: usize) -> Plane {
        let mut p = Plane::new(self.w, self.h);
        for y in 0..self.h {
            for x in 0..self.w {
                let mut s = 0.0;
                for j in 0..3 {
                    for i in 0..3 {
                        s += self.eval_rad(x as f32 + sx + (i as f32 + 0.5) / 3.0 - 0.5, y as f32 + sy + (j as f32 + 0.5) / 3.0 - 0.5, t);
                    }
                }
                p.data[y * self.w + x] = s / 9.0;
            }
        }
        p
    }
}

/// Ráfaga HDR: la foto k usa la exposición `exposures[k % len]` (relativa a la referencia).
/// Codificación gamma 2.2, saturación a 255 y ruido var = a·v + b.
pub fn make_hdr_burst(scene: &Scene, n: usize, exposures: &[f32], a: f32, b: f32, seed: u64) -> Burst {
    let mut rng = Rng::new(seed);
    let mut frames = Vec::new();
    let mut shifts = Vec::new();
    for t in 0..n {
        let e = exposures[t % exposures.len()];
        let (sx, sy) = (rng.range(-3.0, 3.0), rng.range(-3.0, 3.0));
        let rad = scene.render_rad(sx, sy, t);
        let enc = |r: f32| 255.0 * (r * e).clamp(0.0, 1.0).powf(1.0 / 2.2);
        let clean: Vec<f32> = rad.data.iter().map(|&r| enc(r)).collect();
        let luma: Vec<u16> = clean
            .iter()
            .map(|&v| {
                let sigma = (a * v + b).sqrt();
                (v + sigma * rng.gauss()).round().clamp(0.0, 255.0) as u16
            })
            .collect();
        let (cw, ch) = ((scene.w + 1) / 2, (scene.h + 1) / 2);
        let mut cb = vec![0u16; cw * ch];
        let mut cr = vec![0u16; cw * ch];
        for j in 0..ch {
            for i in 0..cw {
                let mut s = 0.0;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let xx = (2 * i + dx).min(scene.w - 1);
                        let yy = (2 * j + dy).min(scene.h - 1);
                        s += clean[yy * scene.w + xx];
                    }
                }
                let v = s / 4.0 - 128.0;
                cb[j * cw + i] = (128.0 + 0.3 * v + 4.0 * rng.gauss()).round().clamp(0.0, 255.0) as u16;
                cr[j * cw + i] = (128.0 - 0.25 * v + 4.0 * rng.gauss()).round().clamp(0.0, 255.0) as u16;
            }
        }
        frames.push(Frame {
            width: scene.w as u32,
            height: scene.h as u32,
            white_level: 255,
            iso: 800,
            exposure_ns: (30_000_000.0 * e as f64) as u64,
            rotation: 0,
            luma,
            cw: cw as u32,
            ch: ch as u32,
            cb,
            cr,
        });
        shifts.push((sx, sy));
    }
    let rots = vec![0.0; frames.len()];
    Burst { frames, shifts, rots }
}

/// Ruido: var = a·v + b (en escala 0..255). `rot_deg` = desviación típica de la rotación por foto.
pub fn make_burst(scene: &Scene, n: usize, a: f32, b: f32, seed: u64, rot_deg: f32, blur_px: f32, lens_sigma: f32) -> Burst {
    let mut rng = Rng::new(seed);
    let mut frames = Vec::new();
    let mut shifts = Vec::new();
    let mut rots = Vec::new();
    for t in 0..n {
        let (sx, sy) = (rng.range(-3.0, 3.0), rng.range(-3.0, 3.0));
        let rot = rng.gauss() * rot_deg;
        let mut clean = scene.render_rot(sx, sy, rot, t);
        // Emborronado óptico del lente (PSF gaussiana), igual en todas las fotos.
        if lens_sigma > 0.05 {
            clean = camara_core::deconv::gauss_blur(&clean, lens_sigma);
        }
        // Movimiento de la mano durante la exposición: desenfoque lineal de longitud y dirección
        // aleatorias (la mayoría de fotos casi nítidas, algunas movidas).
        let len = (rng.gauss() * blur_px).abs();
        if len > 0.2 {
            let ang = rng.range(0.0, std::f32::consts::PI);
            let (dx, dy) = (ang.cos(), ang.sin());
            let src = clean.clone();
            for y in 0..scene.h {
                for x in 0..scene.w {
                    let mut s = 0.0;
                    for k in 0..7 {
                        let o = (k as f32 / 6.0 - 0.5) * len;
                        s += src.sample(x as f32 + dx * o, y as f32 + dy * o);
                    }
                    clean.data[y * scene.w + x] = s / 7.0;
                }
            }
        }
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
        rots.push(rot);
    }
    Burst { frames, shifts, rots }
}
