//! Métricas de calidad.

use crate::image::Plane;
use crate::pyramid::downsample;

/// PSNR entre dos planos ignorando un borde (por zonas sin información tras alinear).
pub fn psnr(a: &Plane, b: &Plane, border: usize, peak: f32) -> f32 {
    let mut se = 0.0f64;
    let mut n = 0u64;
    for y in border..a.h.saturating_sub(border) {
        for x in border..a.w.saturating_sub(border) {
            let d = (a.data[y * a.w + x] - b.data[y * b.w + x]) as f64;
            se += d * d;
            n += 1;
        }
    }
    let mse = (se / n.max(1) as f64).max(1e-12);
    (10.0 * ((peak as f64).powi(2) / mse).log10()) as f32
}

/// Nitidez robusta al ruido: energía del gradiente tras reducir dos veces (promedia el ruido).
pub fn sharpness(p: &Plane) -> f32 {
    let s = downsample(&downsample(p));
    let mut e = 0.0f64;
    for y in 0..s.h.saturating_sub(1) {
        for x in 0..s.w.saturating_sub(1) {
            let c = s.data[y * s.w + x];
            let dx = s.data[y * s.w + x + 1] - c;
            let dy = s.data[(y + 1) * s.w + x] - c;
            e += (dx * dx + dy * dy) as f64;
        }
    }
    (e / (s.w * s.h).max(1) as f64) as f32
}
