//! Deconvolución de Richardson–Lucy: deshace el emborronado óptico (lente, sensor, interpolación)
//! en lugar de solo realzar contraste. Es más fiel que una máscara de nitidez porque modela la
//! causa: la imagen observada = imagen real ⊗ PSF.
//!
//! Al trabajar sobre la imagen YA fusionada (con poco ruido) se puede deconvolucionar con
//! seguridad, algo que sobre una sola foto ruidosa amplificaría el ruido. Se limita el
//! resultado al rango local de la observación para evitar halos.

use crate::image::Plane;
use crate::parallel::par_rows;

fn kernel(sigma: f32) -> Vec<f32> {
    let r = (3.0 * sigma).ceil().max(1.0) as isize;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f32 = k.iter().sum();
    for v in k.iter_mut() {
        *v /= s;
    }
    k
}

/// Desenfoque gaussiano separable (PSF simétrica: la transpuesta es ella misma).
pub fn gauss_blur(p: &Plane, sigma: f32) -> Plane {
    let k = kernel(sigma);
    let r = (k.len() / 2) as isize;
    let mut tmp = Plane::new(p.w, p.h);
    par_rows(&mut tmp.data, p.w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                s += kv * p.get(x as isize + i as isize - r, y as isize);
            }
            *o = s;
        }
    });
    let mut out = Plane::new(p.w, p.h);
    par_rows(&mut out.data, p.w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                s += kv * tmp.get(x as isize, y as isize + i as isize - r);
            }
            *o = s;
        }
    });
    out
}

/// Mínimo y máximo local (ventana (2r+1)²), separables.
fn local_range(p: &Plane, r: isize) -> (Plane, Plane) {
    let (w, h) = (p.w, p.h);
    let mut lo_t = Plane::new(w, h);
    let mut hi_t = Plane::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for d in -r..=r {
                let v = p.get(x as isize + d, y as isize);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            lo_t.data[y * w + x] = lo;
            hi_t.data[y * w + x] = hi;
        }
    }
    let mut lo_o = Plane::new(w, h);
    let mut hi_o = Plane::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for d in -r..=r {
                lo = lo.min(lo_t.get(x as isize, y as isize + d));
                hi = hi.max(hi_t.get(x as isize, y as isize + d));
            }
            lo_o.data[y * w + x] = lo;
            hi_o.data[y * w + x] = hi;
        }
    }
    (lo_o, hi_o)
}

/// Estima el emborronado real de la imagen a partir de sus bordes nítidos: mide el ancho de
/// transición 10 %–90 % (para una PSF gaussiana vale 2.563·σ) en el borde más fuerte de cada
/// bloque y toma la mediana. Devuelve σ en píxeles, o None si no hay bordes suficientes.
pub fn estimate_blur_sigma(p: &Plane) -> Option<f32> {
    let s = gauss_blur(p, 0.8);
    let (w, h) = (p.w, p.h);
    let block = 24usize;
    let mut widths: Vec<f32> = Vec::new();
    let mut by = 4;
    while by + block + 4 < h {
        let mut bx = 4;
        while bx + block + 4 < w {
            // Píxel de mayor gradiente del bloque.
            let (mut best, mut bpx, mut bpy, mut gxx, mut gyy) = (0.0f32, 0usize, 0usize, 0.0f32, 0.0f32);
            for y in by..by + block {
                for x in bx..bx + block {
                    let gx = (s.data[y * w + x + 1] - s.data[y * w + x - 1]) * 0.5;
                    let gy = (s.data[(y + 1) * w + x] - s.data[(y - 1) * w + x]) * 0.5;
                    let m = gx * gx + gy * gy;
                    if m > best {
                        best = m;
                        bpx = x;
                        bpy = y;
                        gxx = gx;
                        gyy = gy;
                    }
                }
            }
            bx += block;
            let mag = best.sqrt();
            if mag < 12.0 {
                continue;
            }
            let (nx, ny) = (gxx / mag, gyy / mag);
            // Perfil a lo largo del gradiente (de -6 a +6 px, paso 0.25).
            let mut prof = [0.0f32; 49];
            let mut ok = true;
            for (i, v) in prof.iter_mut().enumerate() {
                let t = -6.0 + i as f32 * 0.25;
                let (px, py) = (bpx as f32 + nx * t, bpy as f32 + ny * t);
                if px < 1.0 || py < 1.0 || px > (w - 2) as f32 || py > (h - 2) as f32 {
                    ok = false;
                    break;
                }
                *v = p.sample(px, py);
            }
            if !ok {
                continue;
            }
            let med = |a: &[f32]| {
                let mut v = a.to_vec();
                v.sort_by(|x, y| x.partial_cmp(y).unwrap());
                v[v.len() / 2]
            };
            let lo = med(&prof[0..6]);
            let hi = med(&prof[43..49]);
            let c = hi - lo;
            if c.abs() < 35.0 {
                continue;
            }
            // Mesetas planas a ambos lados: es un borde en escalón, no una línea ni una textura.
            let flat = |a: &[f32], m: f32| a.iter().map(|v| (v - m).abs()).fold(0.0f32, f32::max) < 0.18 * c.abs();
            if !flat(&prof[0..6], lo) || !flat(&prof[43..49], hi) {
                continue;
            }
            let norm: Vec<f32> = prof.iter().map(|v| (v - lo) / c).collect();
            let cross = |level: f32| -> Option<f32> {
                for i in 0..48 {
                    if norm[i] < level && norm[i + 1] >= level {
                        return Some(-6.0 + (i as f32 + (level - norm[i]) / (norm[i + 1] - norm[i] + 1e-9)) * 0.25);
                    }
                }
                None
            };
            if let (Some(a), Some(b)) = (cross(0.1), cross(0.9)) {
                let wd = (b - a).abs();
                if wd > 0.8 && wd < 5.0 {
                    widths.push(wd);
                }
            }
        }
        by += block;
    }
    if widths.len() < 15 {
        return None;
    }
    widths.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some((widths[widths.len() / 2] / 2.563).clamp(0.3, 1.5))
}

/// Richardson–Lucy con PSF gaussiana de desviación `sigma` (px). `strength` 0..1 mezcla el
/// resultado con la observación. Los píxeles quedan acotados al rango local ± un margen.
pub fn richardson_lucy(obs: &Plane, sigma: f32, iters: usize, strength: f32) -> Plane {
    const EPS: f32 = 1.0;
    let mut est = obs.clone();
    for _ in 0..iters {
        let blurred = gauss_blur(&est, sigma);
        let mut ratio = Plane::new(obs.w, obs.h);
        for i in 0..ratio.data.len() {
            ratio.data[i] = (obs.data[i] / blurred.data[i].max(EPS)).clamp(0.5, 2.0);
        }
        let corr = gauss_blur(&ratio, sigma);
        for i in 0..est.data.len() {
            est.data[i] *= corr.data[i];
        }
    }
    let (lo, hi) = local_range(obs, 2);
    let mut out = Plane::new(obs.w, obs.h);
    for i in 0..out.data.len() {
        let margin = 0.06 * (hi.data[i] - lo.data[i]);
        let v = obs.data[i] + strength * (est.data[i] - obs.data[i]);
        out.data[i] = v.clamp(lo.data[i] - margin, hi.data[i] + margin);
    }
    out
}
