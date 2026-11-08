//! HDR por exposiciones múltiples.
//!
//! La ráfaga mezcla fotos cortas (protegen las luces), normales y largas (rescatan las
//! sombras). Se trabaja en luz LINEAL: cada foto se lleva a la escala de la referencia
//! dividiendo por su exposición relativa y se promedian ponderando por la fiabilidad de cada
//! muestra (sin saturar, poco ruido, sin movimiento). El resultado es una imagen con más
//! rango dinámico del que cabe en 8 bits.

use crate::image::Plane;
use crate::merge::{MergeParams, Weights};
use crate::noise::NoiseModel;
use crate::package::Frame;
use crate::parallel::{par_rows, par_rows2};

/// Curva de gamma típica de la cámara; se refina midiéndola en cada ráfaga.
pub const GAMMA0: f32 = 2.2;

/// Exposición total de una foto (tiempo × ISO).
pub fn exposure(f: &Frame) -> f64 {
    f.exposure_ns.max(1) as f64 * f.iso.max(1) as f64
}

/// ¿La ráfaga mezcla exposiciones distintas?
pub fn is_bracketed(frames: &[Frame]) -> bool {
    let e0 = exposure(&frames[0]);
    frames.iter().any(|f| (exposure(f) / e0).log2().abs() > 0.4)
}

/// Lleva una foto al brillo de la referencia (en el dominio de la imagen) para poder alinearla.
pub fn normalized(p: &Plane, e: f32, gamma: f32) -> Plane {
    let k = e.powf(-1.0 / gamma);
    let mut out = p.clone();
    for v in out.data.iter_mut() {
        *v = (*v * k).min(400.0);
    }
    out
}

/// Mide la gamma real de la cámara: para píxeles bien expuestos en ambas fotos,
/// (v_k / v_ref)^γ = e_k  →  γ = ln e_k / ln(v_k / v_ref). Mediana robusta.
pub fn estimate_gamma(reference: &Plane, alts: &[(&Plane, f32)]) -> f32 {
    let mut gs: Vec<f32> = Vec::new();
    for (a, e) in alts {
        if e.ln().abs() < 0.2 {
            continue;
        }
        let stride = (reference.data.len() / 200_000).max(1);
        let mut i = 0;
        while i < reference.data.len() {
            let (vr, va) = (reference.data[i], a.data[i]);
            if vr > 35.0 && vr < 215.0 && va > 35.0 && va < 215.0 {
                let lr = (va / vr).ln();
                if lr.abs() > 0.12 {
                    let g = e.ln() / lr;
                    if g > 0.8 && g < 4.5 {
                        gs.push(g);
                    }
                }
            }
            i += stride;
        }
    }
    if gs.len() < 500 {
        return GAMMA0;
    }
    gs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    gs[gs.len() / 2].clamp(1.4, 3.0)
}

/// Fusión HDR de luminancia. `planes` = todas las fotos ya alineadas a la referencia (la
/// referencia incluida, en su posición), `e` = exposición de cada una relativa a la referencia.
/// Devuelve la luminancia HDR en la escala "gamma de la referencia" (puede superar 255) y los
/// pesos normalizados de cada foto (suman 1), reutilizables para el color.
pub fn merge_hdr(planes: &[&Plane], e: &[f32], ref_idx: usize, noise: &NoiseModel, gamma: f32, p: &MergeParams) -> (Plane, Weights) {
    let (w, h, n) = (planes[0].w, planes[0].h, planes.len());
    let rp = planes[ref_idx];
    let inv_g = 1.0 / gamma;
    let gscale: Vec<f32> = e.iter().map(|&x| x.powf(-inv_g)).collect();
    let shortest = (0..n).min_by(|&a, &b| e[a].partial_cmp(&e[b]).unwrap()).unwrap();

    let mut out = Plane::new(w, h);
    let mut wdata = vec![0u8; w * h * n];
    par_rows2(&mut out.data, w, &mut wdata, w * n, |y, row, wrow| {
        let yi = y as isize;
        let mut ws = vec![0f32; n];
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let xi = x as isize;
            let vref = rp.data[i];
            let mut num = 0.0f32;
            let mut den = 0.0f32;
            for k in 0..n {
                let v = planes[k].data[i];
                // Una muestra cerca del tope SUBESTIMA la luz real (el ruido también se recorta en
                // 255), así que su peso cae a cero antes de llegar ahí.
                // La decisión se toma con el valor LOCAL (media 3x3), no con el píxel ruidoso:
                // si no, algunos píxeles de una zona saturada caerían bajo el umbral por el ruido.
                let mut vs = 0.0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        vs += planes[k].get(xi + dx, yi + dy);
                    }
                }
                vs /= 9.0;
                let clipw = if vs >= 228.0 {
                    0.0
                } else if vs > 205.0 {
                    let t = (228.0 - vs) / 23.0;
                    t * t
                } else {
                    1.0
                };
                // Peso = 1 / varianza del ruido expresado en luz lineal y en la escala de la referencia.
                let ve = v.max(8.0);
                let sv2 = noise.var(ve.min(255.0)) + 0.083;
                let dl = gamma * (ve / 255.0).powf(gamma - 1.0) / 255.0;
                let var_rad = dl * dl * sv2 / (e[k] * e[k]);
                let mut wgt = clipw / var_rad;

                // Rechazo de movimiento donde la referencia es fiable.
                if k != ref_idx && clipw > 0.0 && vref > 8.0 && vref < 225.0 {
                    let mut ms = 0.0;
                    let mut cnt = 0.0;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let r = rp.get(xi + dx, yi + dy);
                            let a = planes[k].get(xi + dx, yi + dy);
                            if r > 8.0 && r < 232.0 && a > 8.0 && a < 232.0 {
                                let d = a * gscale[k] - r;
                                ms += d * d;
                                cnt += 1.0;
                            }
                        }
                    }
                    if cnt >= 4.0 {
                        ms /= cnt;
                        let var_exp = noise.var(vref) + noise.var(v.min(255.0)) * gscale[k] * gscale[k];
                        let t = ms / var_exp;
                        let excess = ((t - p.t0) / p.scale).max(0.0);
                        wgt *= 1.0 / (1.0 + excess * excess);
                    }
                }
                let rad = (v / 255.0).powf(gamma) / e[k];
                num += wgt * rad;
                den += wgt;
                ws[k] = wgt;
            }
            let rad = if den > 1e-20 {
                num / den
            } else {
                (planes[shortest].data[i] / 255.0).powf(gamma) / e[shortest]
            };
            *o = 255.0 * rad.max(0.0).powf(inv_g);
            for k in 0..n {
                let f = if den > 1e-20 { ws[k] / den } else if k == shortest { 1.0 } else { 0.0 };
                wrow[k * w + x] = (f * 255.0).round() as u8;
            }
        }
    });
    (out, Weights { w, n, data: wdata })
}

/// Fusiona un plano de croma (media resolución) con los pesos HDR de la luminancia.
pub fn merge_chroma_hdr(chroma: &[&Plane], weights: &Weights, luma_h: usize, ref_idx: usize) -> Plane {
    let (cw, ch) = (chroma[0].w, chroma[0].h);
    let lw = weights.w;
    let mut out = Plane::new(cw, ch);
    par_rows(&mut out.data, cw, |j, row| {
        for (i, o) in row.iter_mut().enumerate() {
            let mut acc = 0.0;
            let mut ws = 0.0;
            for (k, c) in chroma.iter().enumerate() {
                let mut wt = 0.0;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let lx = (2 * i + dx).min(lw - 1);
                        let ly = (2 * j + dy).min(luma_h - 1);
                        wt += weights.get(k, lx, ly);
                    }
                }
                wt /= 4.0;
                acc += wt * c.data[j * cw + i];
                ws += wt;
            }
            *o = if ws > 1e-3 { acc / ws } else { chroma[ref_idx].data[j * cw + i] };
        }
    });
    out
}
