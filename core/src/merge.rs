//! Fusión robusta de fotos alineadas.
//!
//! Para cada píxel y cada foto se compara la diferencia cuadrática local (3x3) con la
//! esperada solo por ruido. Si coincide, la foto se promedia con peso 1; si hay
//! movimiento o desalineación, el peso cae a ~0 (sin fantasmas).

use crate::image::Plane;
use crate::noise::NoiseModel;
use crate::parallel::{par_rows, par_rows2};

#[derive(Clone, Debug)]
pub struct MergeParams {
    /// Umbral (en múltiplos de la varianza esperada) a partir del cual empieza a caer el peso.
    pub t0: f32,
    /// Anchura de la caída.
    pub scale: f32,
}

impl Default for MergeParams {
    fn default() -> Self {
        Self { t0: 1.7, scale: 1.0 }
    }
}

/// Mapas de pesos (0..255) de todas las fotos alineadas, organizados por fila:
/// índice = (y * n_fotos + k) * ancho + x. Se reutilizan para fusionar el color.
pub struct Weights {
    pub w: usize,
    pub n: usize,
    pub data: Vec<u8>,
}

impl Weights {
    #[inline]
    pub fn get(&self, k: usize, x: usize, y: usize) -> f32 {
        self.data[(y * self.n + k) * self.w + x] as f32 / 255.0
    }
}

pub fn merge(reference: &Plane, aligned: &[Plane], noise: &NoiseModel, p: &MergeParams) -> (Plane, Weights) {
    let w = reference.w;
    let h = reference.h;
    let na = aligned.len();
    let mut out = Plane::new(w, h);
    let mut wdata = vec![0u8; w * h * na];
    par_rows2(&mut out.data, w, &mut wdata, w * na, |y, row, wrow| {
        let yi = y as isize;
        for (x, o) in row.iter_mut().enumerate() {
            let xi = x as isize;
            let r = reference.data[y * w + x];

            let mut mean = 0.0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    mean += reference.get(xi + dx, yi + dy);
                }
            }
            mean /= 9.0;
            let var = noise.var(mean);

            let mut acc = r;
            let mut wsum = 1.0;
            for (k, a) in aligned.iter().enumerate() {
                let mut ms = 0.0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let d = a.get(xi + dx, yi + dy) - reference.get(xi + dx, yi + dy);
                        ms += d * d;
                    }
                }
                ms /= 9.0;
                let t = ms / (2.0 * var);
                let excess = ((t - p.t0) / p.scale).max(0.0);
                let wt = 1.0 / (1.0 + excess * excess);
                acc += wt * a.data[y * w + x];
                wsum += wt;
                wrow[k * w + x] = (wt * 255.0).round() as u8;
            }
            *o = acc / wsum;
        }
    });
    (out, Weights { w, n: na, data: wdata })
}

/// Fusiona un plano de croma (media resolución) con los pesos calculados con la luminancia.
pub fn merge_chroma(reference: &Plane, aligned: &[Plane], weights: &Weights, luma_h: usize) -> Plane {
    let (cw, ch) = (reference.w, reference.h);
    let lw = weights.w;
    let mut out = Plane::new(cw, ch);
    par_rows(&mut out.data, cw, |j, row| {
        for (i, o) in row.iter_mut().enumerate() {
            let mut acc = reference.data[j * cw + i];
            let mut wsum = 1.0f32;
            for (k, a) in aligned.iter().enumerate() {
                let mut ws = 0.0;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let lx = (2 * i + dx).min(lw - 1);
                        let ly = (2 * j + dy).min(luma_h - 1);
                        ws += weights.get(k, lx, ly);
                    }
                }
                let wt = ws / 4.0;
                acc += wt * a.data[j * cw + i];
                wsum += wt;
            }
            *o = acc / wsum;
        }
    });
    out
}
