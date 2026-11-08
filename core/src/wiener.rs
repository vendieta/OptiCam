//! Fusión temporal por frecuencias con filtro de Wiener (estilo HDR+).
//!
//! La imagen se divide en bloques de 16x16 que se solapan al 50 % con una ventana de coseno
//! al cuadrado (las ventanas suman exactamente 1). En cada bloque se calcula la FFT 2D de la
//! referencia y de cada foto ya alineada, y para CADA FRECUENCIA se decide cuánto fiarse de
//! cada foto:
//!
//!     D = T_ref − T_z            (diferencia entre referencia y foto z)
//!     A = |D|² / (|D|² + c·σ²)    (0 = igual salvo ruido → promediar; 1 = distinto → usar referencia)
//!     resultado = (1/N)·( T_ref + Σ_z [ T_z + A·(T_ref − T_z) ] )
//!
//! Las frecuencias en las que las fotos coinciden (zonas estáticas y planas) se promedian a
//! fondo; las que difieren por movimiento o desalineación recurren a la referencia. Así se
//! conserva más detalle que decidiendo píxel a píxel.

use crate::image::Plane;
use crate::merge::Weights;
use crate::noise::NoiseModel;
use crate::parallel::par_rows;

const T: usize = 16;
const S: usize = 8; // paso entre bloques (50 % de solape)
const N2: usize = T * T;

const COS: [f32; 8] = [1.0, 0.923_879_5, 0.707_106_8, 0.382_683_43, 0.0, -0.382_683_43, -0.707_106_8, -0.923_879_5];
const SIN: [f32; 8] = [0.0, 0.382_683_43, 0.707_106_8, 0.923_879_5, 1.0, 0.923_879_5, 0.707_106_8, 0.382_683_43];
const REV: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];

/// FFT de 16 puntos en sitio (radix-2). `inverse` usa el signo contrario (sin normalizar).
fn fft16(re: &mut [f32; 16], im: &mut [f32; 16], inverse: bool) {
    let mut r = [0.0f32; 16];
    let mut i = [0.0f32; 16];
    for k in 0..16 {
        r[k] = re[REV[k]];
        i[k] = im[REV[k]];
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= 16 {
        let half = len / 2;
        let step = 16 / len;
        let mut start = 0;
        while start < 16 {
            for j in 0..half {
                let k = j * step;
                let (wr, wi) = (COS[k], sign * SIN[k]);
                let a = start + j;
                let b = a + half;
                let tr = r[b] * wr - i[b] * wi;
                let ti = r[b] * wi + i[b] * wr;
                r[b] = r[a] - tr;
                i[b] = i[a] - ti;
                r[a] += tr;
                i[a] += ti;
            }
            start += len;
        }
        len *= 2;
    }
    *re = r;
    *im = i;
}

/// FFT 2D de un bloque 16x16 (filas y luego columnas).
fn fft2d(re: &mut [f32; N2], im: &mut [f32; N2], inverse: bool) {
    let mut rr = [0.0f32; 16];
    let mut ii = [0.0f32; 16];
    for row in 0..T {
        rr.copy_from_slice(&re[row * T..row * T + T]);
        ii.copy_from_slice(&im[row * T..row * T + T]);
        fft16(&mut rr, &mut ii, inverse);
        re[row * T..row * T + T].copy_from_slice(&rr);
        im[row * T..row * T + T].copy_from_slice(&ii);
    }
    for col in 0..T {
        for row in 0..T {
            rr[row] = re[row * T + col];
            ii[row] = im[row * T + col];
        }
        fft16(&mut rr, &mut ii, inverse);
        for row in 0..T {
            re[row * T + col] = rr[row];
            im[row * T + col] = ii[row];
        }
    }
}

/// Ventana sin²: w[i] + w[i+8] = 1.
fn window() -> [f32; T] {
    let mut w = [0.0f32; T];
    for (i, v) in w.iter_mut().enumerate() {
        let s = (std::f32::consts::PI * (i as f32 + 0.5) / T as f32).sin();
        *v = s * s;
    }
    w
}

fn load_tile(p: &Plane, ox: isize, oy: isize, win: &[f32; T], re: &mut [f32; N2], im: &mut [f32; N2]) {
    for j in 0..T {
        for i in 0..T {
            re[j * T + i] = p.get(ox + i as isize, oy + j as isize) * win[j] * win[i];
            im[j * T + i] = 0.0;
        }
    }
}

struct Strip {
    /// Acumulador de la franja: T filas × `sw` columnas (x desplazada +S).
    data: Vec<f32>,
    /// Peso medio (1−A) de cada foto en cada bloque de la fila: [bloque][foto].
    tile_w: Vec<f32>,
}

/// Fusión Wiener. `c` = tolerancia al ruido (mayor = promedia más agresivamente; ~8 es típico).
/// Devuelve la luminancia fusionada y mapas de pesos por foto (para fusionar el color).
pub fn merge_wiener(reference: &Plane, aligned: &[Plane], noise: &NoiseModel, c: f32) -> (Plane, Weights) {
    let (w, h) = (reference.w, reference.h);
    let na = aligned.len();
    let n_total = (na + 1) as f32;
    let ntx = w / S + 2;
    let nty = h / S + 2;
    let sw = S * ntx + S;
    let win = window();
    // Σ w² por dimensión = 6 para esta ventana → varianza del ruido por frecuencia = σ²·36.
    let win_energy: f32 = win.iter().map(|v| v * v).sum::<f32>();
    let noise_scale = win_energy * win_energy;

    let mut strips: Vec<Strip> = (0..nty).map(|_| Strip { data: Vec::new(), tile_w: Vec::new() }).collect();
    par_rows(&mut strips, 1, |ty, slot| {
        let oy = -(S as isize) + (ty * S) as isize;
        let mut data = vec![0.0f32; T * sw];
        let mut tile_w = vec![0.0f32; ntx * na];
        let mut t0r = [0.0f32; N2];
        let mut t0i = [0.0f32; N2];
        let mut tzr = [0.0f32; N2];
        let mut tzi = [0.0f32; N2];
        let mut accr = [0.0f32; N2];
        let mut acci = [0.0f32; N2];
        for tx in 0..ntx {
            let ox = -(S as isize) + (tx * S) as isize;
            load_tile(reference, ox, oy, &win, &mut t0r, &mut t0i);
            fft2d(&mut t0r, &mut t0i, false);
            // Ruido esperado según la intensidad media del bloque de la referencia.
            let mut mean = 0.0;
            for j in 0..T {
                for i in 0..T {
                    mean += reference.get(ox + i as isize, oy + j as isize);
                }
            }
            mean /= N2 as f32;
            let sf2 = noise.var(mean) * noise_scale;
            let thr = c * sf2;

            accr.fill(0.0);
            acci.fill(0.0);
            for (z, a) in aligned.iter().enumerate() {
                load_tile(a, ox, oy, &win, &mut tzr, &mut tzi);
                fft2d(&mut tzr, &mut tzi, false);
                let mut wsum = 0.0;
                for k in 0..N2 {
                    let dr = t0r[k] - tzr[k];
                    let di = t0i[k] - tzi[k];
                    let d2 = dr * dr + di * di;
                    let aa = d2 / (d2 + thr);
                    accr[k] += tzr[k] + aa * dr;
                    acci[k] += tzi[k] + aa * di;
                    wsum += 1.0 - aa;
                }
                tile_w[tx * na + z] = wsum / N2 as f32;
            }
            for k in 0..N2 {
                accr[k] = (t0r[k] + accr[k]) / n_total;
                acci[k] = (t0i[k] + acci[k]) / n_total;
            }
            fft2d(&mut accr, &mut acci, true);
            let inv = 1.0 / N2 as f32;
            let base_x = tx * S; // posición en la franja (ox + S)
            for j in 0..T {
                for i in 0..T {
                    data[j * sw + base_x + i] += accr[j * T + i] * inv;
                }
            }
        }
        slot[0] = Strip { data, tile_w };
    });

    // Suma de las franjas (cada una cubre T filas empezando en oy).
    let mut out = Plane::new(w, h);
    for (ty, st) in strips.iter().enumerate() {
        let oy = -(S as isize) + (ty * S) as isize;
        for j in 0..T {
            let y = oy + j as isize;
            if y < 0 || y >= h as isize {
                continue;
            }
            let row = &mut out.data[y as usize * w..(y as usize + 1) * w];
            let src = &st.data[j * sw + S..j * sw + S + w];
            for (o, &v) in row.iter_mut().zip(src) {
                *o += v;
            }
        }
    }

    // Pesos por píxel: interpolación bilineal de la rejilla de bloques (centros en múltiplos de S).
    let mut wdata = vec![0u8; w * h * na];
    par_rows(&mut wdata, w * na, |y, row| {
        let gy = y as f32 / S as f32;
        let ty0 = (gy.floor() as usize).min(nty - 1);
        let ty1 = (ty0 + 1).min(nty - 1);
        let fy = gy - ty0 as f32;
        for x in 0..w {
            let gx = x as f32 / S as f32;
            let tx0 = (gx.floor() as usize).min(ntx - 1);
            let tx1 = (tx0 + 1).min(ntx - 1);
            let fx = gx - tx0 as f32;
            for k in 0..na {
                let g = |ty: usize, tx: usize| strips[ty].tile_w[tx * na + k];
                let v = (g(ty0, tx0) * (1.0 - fx) + g(ty0, tx1) * fx) * (1.0 - fy) + (g(ty1, tx0) * (1.0 - fx) + g(ty1, tx1) * fx) * fy;
                row[k * w + x] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    });
    (out, Weights { w, n: na, data: wdata })
}
