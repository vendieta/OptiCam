//! Color: reducción de ruido del croma guiada por la luminancia, YUV→RGB y rotación.

use crate::image::Plane;
use crate::parallel::par_rows;

/// Filtro bilateral conjunto: suaviza el croma sin cruzar bordes de la luminancia.
/// `guide` debe tener la misma resolución que `c` (luminancia reducida a la mitad).
pub fn denoise_chroma(c: &Plane, guide: &Plane, range_sigma: f32, spatial_sigma: f32, radius: isize) -> Plane {
    let mut out = Plane::new(c.w, c.h);
    let inv_r = 1.0 / (2.0 * range_sigma * range_sigma);
    let inv_s = 1.0 / (2.0 * spatial_sigma * spatial_sigma);
    par_rows(&mut out.data, c.w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let g0 = guide.data[y * c.w + x];
            let mut acc = 0.0;
            let mut ws = 0.0;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let g = guide.get(x as isize + dx, y as isize + dy);
                    let dg = g - g0;
                    let w = (-(dg * dg) * inv_r - ((dx * dx + dy * dy) as f32) * inv_s).exp();
                    acc += w * c.get(x as isize + dx, y as isize + dy);
                    ws += w;
                }
            }
            *o = acc / ws;
        }
    });
    out
}

/// Reduce a la mitad promediando 2x2 (guía para el croma).
pub fn half(p: &Plane, cw: usize, ch: usize) -> Plane {
    let mut out = Plane::new(cw, ch);
    for j in 0..ch {
        for i in 0..cw {
            let mut s = 0.0;
            for dy in 0..2 {
                for dx in 0..2 {
                    s += p.get((2 * i + dx) as isize, (2 * j + dy) as isize);
                }
            }
            out.data[j * cw + i] = s / 4.0;
        }
    }
    out
}

/// YUV (BT.601 rango completo, como la cámara de Android) → RGB 8 bits, con rotación horaria.
/// Devuelve (ancho, alto, píxeles RGB).
pub fn to_rgb(y: &Plane, cb: &Plane, cr: &Plane, white: f32, rotation: u32) -> (usize, usize, Vec<u8>) {
    let (w, h) = (y.w, y.h);
    let k = 255.0 / white;
    let mut rgb = vec![0u8; w * h * 3];
    par_rows(&mut rgb, w * 3, |py, row| {
        for x in 0..w {
            let yy = y.data[py * w + x] * k;
            // El croma está centrado entre los píxeles de luminancia.
            let cx = (x as f32 - 0.5) * 0.5;
            let cy = (py as f32 - 0.5) * 0.5;
            let b = cb.sample(cx, cy) * k - 128.0;
            let r = cr.sample(cx, cy) * k - 128.0;
            let rr = yy + 1.402 * r;
            let gg = yy - 0.344_136 * b - 0.714_136 * r;
            let bb = yy + 1.772 * b;
            // Tramado triangular: evita las bandas en degradados oscuros.
            let d = dither(x as u32, py as u32);
            row[x * 3] = (rr + d).round().clamp(0.0, 255.0) as u8;
            row[x * 3 + 1] = (gg + d).round().clamp(0.0, 255.0) as u8;
            row[x * 3 + 2] = (bb + d).round().clamp(0.0, 255.0) as u8;
        }
    });
    rotate_rgb(w, h, rgb, rotation)
}

#[inline]
fn dither(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    let a = (h & 0xFFFF) as f32 / 65535.0;
    let b = (h >> 16) as f32 / 65535.0;
    (a + b - 1.0) * 0.5
}

/// Rota una imagen RGB en múltiplos de 90° (sentido horario).
pub fn rotate_rgb(w: usize, h: usize, src: Vec<u8>, rotation: u32) -> (usize, usize, Vec<u8>) {
    match rotation % 360 {
        90 => {
            let mut dst = vec![0u8; src.len()];
            let (nw, _nh) = (h, w);
            for y in 0..h {
                for x in 0..w {
                    // (x,y) -> (h-1-y, x)
                    let d = (x * nw + (h - 1 - y)) * 3;
                    let s = (y * w + x) * 3;
                    dst[d..d + 3].copy_from_slice(&src[s..s + 3]);
                }
            }
            (h, w, dst)
        }
        180 => {
            let mut dst = vec![0u8; src.len()];
            for y in 0..h {
                for x in 0..w {
                    let d = ((h - 1 - y) * w + (w - 1 - x)) * 3;
                    let s = (y * w + x) * 3;
                    dst[d..d + 3].copy_from_slice(&src[s..s + 3]);
                }
            }
            (w, h, dst)
        }
        270 => {
            let mut dst = vec![0u8; src.len()];
            let nw = h;
            for y in 0..h {
                for x in 0..w {
                    // (x,y) -> (y, w-1-x)
                    let d = ((w - 1 - x) * nw + y) * 3;
                    let s = (y * w + x) * 3;
                    dst[d..d + 3].copy_from_slice(&src[s..s + 3]);
                }
            }
            (h, w, dst)
        }
        _ => (w, h, src),
    }
}
