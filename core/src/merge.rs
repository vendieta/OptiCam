//! Fusión robusta de fotos alineadas.
//!
//! Para cada píxel y cada foto se compara la diferencia cuadrática local (3x3) con la
//! esperada solo por ruido. Si coincide, la foto se promedia con peso 1; si hay
//! movimiento o desalineación, el peso cae a ~0 (sin fantasmas).

use crate::develop::{blur5, box_blur};
use crate::image::Plane;
use crate::noise::NoiseModel;
use crate::parallel::{par_rows, par_rows2};

#[derive(Clone, Debug)]
pub struct MergeParams {
    /// Umbral (en múltiplos de la varianza esperada) a partir del cual empieza a caer el peso.
    pub t0: f32,
    /// Anchura de la caída.
    pub scale: f32,
    /// Fusión por frecuencias con filtro de Wiener (si no, fusión espacial por píxel).
    pub wiener: bool,
    /// Tolerancia al ruido del filtro de Wiener (mayor = promedia más).
    pub c: f32,
    /// Peso por nitidez: una foto más borrosa que la referencia en una zona pesa menos ahí
    /// (exponente; 0 = desactivado). Evita que las fotos movidas diluyan el detalle fino.
    pub sharp_weight: f32,
}

impl Default for MergeParams {
    fn default() -> Self {
        Self { t0: 1.7, scale: 1.0, wiener: false, c: 8.0, sharp_weight: 3.0 }
    }
}

/// Relación de detalle (foto / referencia) por debajo de la cual una foto empieza a perder peso.
const SHARP_TOLERANCE: f32 = 0.88;

/// Energía de detalle fino local (banda ~1 px) con la parte debida al ruido restada:
/// mide cuánta estructura real hay en cada zona, sin que el ruido la infle.
fn detail_energy(p: &Plane, nv: &Plane) -> Plane {
    // Un desenfoque gaussiano 5x5 deja una banda cuyo ruido blanco tiene 0.79·σ² de varianza.
    const NOISE_FACTOR: f32 = 0.79;
    let blur = blur5(p);
    let mut sq = Plane::new(p.w, p.h);
    for i in 0..sq.data.len() {
        let d = p.data[i] - blur.data[i];
        sq.data[i] = d * d;
    }
    let mut e = box_blur(&sq, 6);
    for i in 0..e.data.len() {
        e.data[i] = (e.data[i] - NOISE_FACTOR * nv.data[i]).max(0.0);
    }
    e
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

    // Mapas de detalle fino (solo si el peso por nitidez está activo).
    let use_sharp = p.sharp_weight > 0.0 && na > 0;
    let (e_ref, e_alt, nv_map) = if use_sharp {
        let mean = box_blur(reference, 3);
        let mut nv = Plane::new(w, h);
        for i in 0..nv.data.len() {
            nv.data[i] = noise.var(mean.data[i]);
        }
        let er = detail_energy(reference, &nv);
        let ea: Vec<Plane> = aligned.iter().map(|a| detail_energy(a, &nv)).collect();
        (er, ea, nv)
    } else {
        (Plane::new(0, 0), Vec::new(), Plane::new(0, 0))
    };

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
                let mut wt = 1.0 / (1.0 + excess * excess);
                // Peso por nitidez: si la foto es más borrosa que la referencia en esta zona,
                // aporta menos. Solo donde la referencia tiene detalle medible sobre el ruido.
                if use_sharp {
                    let er = e_ref.data[y * w + x];
                    if er > 0.5 * 0.79 * nv_map.data[y * w + x] {
                        let ratio = e_alt[k].data[y * w + x] / er;
                        // Zona muerta: las fluctuaciones normales del ruido no se penalizan;
                        // solo una foto claramente más borrosa pierde peso.
                        if ratio < SHARP_TOLERANCE {
                            wt *= (ratio / SHARP_TOLERANCE).powf(p.sharp_weight);
                        }
                    }
                }
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
