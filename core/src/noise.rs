//! Modelo de ruido dependiente de la intensidad: var(v) = a*v + b
//! (ruido de disparo + ruido de lectura), estimado de la propia foto de referencia.

use crate::image::Plane;

#[derive(Clone, Copy, Debug)]
pub struct NoiseModel {
    pub a: f32,
    pub b: f32,
}

impl NoiseModel {
    #[inline]
    pub fn var(&self, v: f32) -> f32 {
        (self.a * v.max(0.0) + self.b).max(0.25)
    }
}

/// Estima el ruido TEMPORAL real comparando la referencia con las fotos ya alineadas.
///
/// Es la medida correcta para decidir cuánto se puede promediar: en cada nivel de intensidad
/// se toma un cuantil bajo (robusto al movimiento y a bordes mal alineados) de la diferencia
/// cuadrática entre fotos. Para ruido gaussiano, el cuantil 0.35 de d² vale 0.206·(σ₁²+σ₂²), y
/// la interpolación de la foto alineada atenúa su ruido (≈0.8·σ²).
pub fn estimate_temporal(reference: &Plane, aligned: &[Plane]) -> NoiseModel {
    const BINS: usize = 16;
    if aligned.is_empty() {
        return estimate(reference);
    }
    let vmax = reference.data.iter().cloned().fold(1.0f32, f32::max);
    let (w, h) = (reference.w as isize, reference.h as isize);
    let stride = ((reference.data.len() * aligned.len().min(4)) / 600_000).max(1);
    let mut samples: Vec<Vec<f32>> = vec![Vec::new(); BINS];
    let mut sums = vec![0.0f64; BINS];
    let mut counter = 0usize;
    for a in aligned.iter().take(4) {
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                counter += 1;
                if counter % stride != 0 {
                    continue;
                }
                let i = (y * w + x) as usize;
                let mut mean = 0.0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        mean += reference.data[((y + dy) * w + x + dx) as usize];
                    }
                }
                mean /= 9.0;
                let d = a.data[i] - reference.data[i];
                let bin = ((mean / vmax) * BINS as f32).clamp(0.0, BINS as f32 - 1.0) as usize;
                samples[bin].push(d * d);
                sums[bin] += mean as f64;
            }
        }
    }
    let mut pts: Vec<(f64, f64, f64)> = Vec::new();
    for b in 0..BINS {
        let s = &mut samples[b];
        if s.len() < 400 {
            continue;
        }
        s.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let q = s[(s.len() as f32 * 0.35) as usize] as f64;
        let var = q / (0.206 * 1.8);
        pts.push((sums[b] / s.len() as f64, var, s.len() as f64));
    }
    fit_line(&pts)
}

/// Ajuste por mínimos cuadrados ponderados de var = a·v + b.
fn fit_line(pts: &[(f64, f64, f64)]) -> NoiseModel {
    if pts.len() < 2 {
        let v = pts.first().map(|p| p.1).unwrap_or(4.0) as f32;
        return NoiseModel { a: 0.0, b: v.max(0.25) };
    }
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for &(x, y, w) in pts {
        sw += w;
        sx += w * x;
        sy += w * y;
        sxx += w * x * x;
        sxy += w * x * y;
    }
    let den = sw * sxx - sx * sx;
    let mut a = if den.abs() > 1e-9 { (sw * sxy - sx * sy) / den } else { 0.0 };
    a = a.max(0.0);
    let b = ((sy - a * sx) / sw).max(0.25);
    NoiseModel { a: a as f32, b: b as f32 }
}

/// Estimación robusta: respuesta del máscara de Immerkaer (insensible a la señal suave),
/// agrupada por intensidad local; cuantil bajo para ignorar bordes y texturas.
pub fn estimate(p: &Plane) -> NoiseModel {
    const BINS: usize = 16;
    let vmax = p.data.iter().cloned().fold(1.0f32, f32::max);
    let mut samples: Vec<Vec<f32>> = vec![Vec::new(); BINS];
    let mut sums = vec![0.0f64; BINS];

    let w = p.w as isize;
    let h = p.h as isize;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let g = |dx: isize, dy: isize| p.data[((y + dy) * w + x + dx) as usize];
            let c = g(-1, -1) - 2.0 * g(0, -1) + g(1, -1) - 2.0 * g(-1, 0) + 4.0 * g(0, 0) - 2.0 * g(1, 0)
                + g(-1, 1)
                - 2.0 * g(0, 1)
                + g(1, 1);
            let mean = (g(-1, -1) + g(0, -1) + g(1, -1) + g(-1, 0) + g(0, 0) + g(1, 0) + g(-1, 1) + g(0, 1) + g(1, 1)) / 9.0;
            let bin = ((mean / vmax) * BINS as f32).clamp(0.0, BINS as f32 - 1.0) as usize;
            samples[bin].push(c.abs());
            sums[bin] += mean as f64;
        }
    }

    // Cuantil 0.35 de |N(0,s²)| ≈ 0.4538·s, con s = 6σ (norma de la máscara).
    let mut pts: Vec<(f64, f64, f64)> = Vec::new(); // (v, var, peso)
    for b in 0..BINS {
        let s = &mut samples[b];
        if s.len() < 500 {
            continue;
        }
        s.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let q = s[(s.len() as f32 * 0.35) as usize] as f64;
        let sigma = q / (0.4538 * 6.0);
        pts.push((sums[b] / s.len() as f64, sigma * sigma, s.len() as f64));
    }
    if pts.len() < 2 {
        let v = pts.first().map(|p| p.1).unwrap_or(4.0) as f32;
        return NoiseModel { a: 0.0, b: v.max(0.25) };
    }
    // Mínimos cuadrados ponderados: var = a*v + b.
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for &(x, y, w) in &pts {
        sw += w;
        sx += w * x;
        sy += w * y;
        sxx += w * x * x;
        sxy += w * x * y;
    }
    let den = sw * sxx - sx * sx;
    let mut a = if den.abs() > 1e-9 { (sw * sxy - sx * sy) / den } else { 0.0 };
    a = a.max(0.0);
    let b = ((sy - a * sx) / sw).max(0.25);
    NoiseModel { a: a as f32, b: b as f32 }
}
