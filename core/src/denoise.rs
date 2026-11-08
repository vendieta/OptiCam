//! Eliminación de ruido multi-escala (wavelets "à trous", todo propio).
//!
//! La imagen se descompone en bandas de detalle de escala 1, 2, 4, 8… píxeles. El ruido de un
//! sensor no es solo "puntitos": también forma manchas de varios píxeles que un filtro de
//! 3x3 no ve. En cada banda se encogen los coeficientes pequeños (ruido) y se conservan los
//! grandes (bordes y texturas reales).

use crate::image::Plane;
use crate::parallel::par_rows;

/// Desviación del ruido blanco en cada banda del filtro B3-spline à trous (2D), relativa a σ.
const BAND_STD: [f32; 6] = [0.889, 0.200, 0.0856, 0.0413, 0.0203, 0.0101];

const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// Desenfoque B3-spline separable con "agujeros" de `step` píxeles entre muestras.
fn atrous(p: &Plane, step: isize) -> Plane {
    let mut tmp = Plane::new(p.w, p.h);
    par_rows(&mut tmp.data, p.w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (k, kv) in K.iter().enumerate() {
                s += kv * p.get(x as isize + (k as isize - 2) * step, y as isize);
            }
            *o = s;
        }
    });
    let mut out = Plane::new(p.w, p.h);
    par_rows(&mut out.data, p.w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (k, kv) in K.iter().enumerate() {
                s += kv * tmp.get(x as isize, y as isize + (k as isize - 2) * step);
            }
            *o = s;
        }
    });
    out
}

/// Estima la σ del ruido con la mediana de la primera banda de detalle (estimador de Donoho):
/// robusto frente a bordes y texturas, que son pocos coeficientes grandes.
pub fn estimate_sigma(p: &Plane) -> f32 {
    let b = atrous(p, 1);
    let step = (p.data.len() / 200_000).max(1);
    let mut v: Vec<f32> = (0..p.data.len()).step_by(step).map(|i| (p.data[i] - b.data[i]).abs()).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = v[v.len() / 2];
    (med / 0.6745 / BAND_STD[0]).max(0.2)
}

/// Contracción "garrote" no negativa: elimina lo que queda bajo el umbral y deja casi intacto
/// lo que lo supera con holgura (conserva mejor los bordes que el umbral blando).
#[inline]
fn shrink(d: f32, t: f32) -> f32 {
    let a2 = d * d;
    let t2 = t * t;
    if a2 <= t2 {
        0.0
    } else {
        d * (1.0 - t2 / a2)
    }
}

/// Elimina ruido de `y`.
///
/// * `sigma` — σ del ruido de entrada.
/// * `scale` — mapa opcional (resolución 1/4, tal como lo produce `downsample` x2) que
///   multiplica σ localmente: el mapeo de tonos amplifica el ruido más en unas zonas que en otras.
/// * `k` — fuerza: múltiplo de σ usado como umbral (≈1.5 suave, ≈3 fuerte).
pub fn wavelet_denoise(y: &Plane, sigma: f32, scale: Option<&Plane>, k: f32, levels: usize) -> Plane {
    let levels = levels.min(BAND_STD.len());
    let mut cur = y.clone();
    let mut recon_details: Vec<Plane> = Vec::with_capacity(levels);
    for j in 0..levels {
        let blurred = atrous(&cur, 1 << j);
        let mut d = Plane::new(y.w, y.h);
        // Menos agresivo en las bandas gruesas: ahí viven las formas reales de la imagen.
        let kj = k * [1.0, 1.0, 0.85, 0.7, 0.6, 0.5][j];
        let tj = kj * sigma * BAND_STD[j];
        par_rows(&mut d.data, y.w, |yy, row| {
            for (x, o) in row.iter_mut().enumerate() {
                let i = yy * y.w + x;
                let raw = cur.data[i] - blurred.data[i];
                let s = match scale {
                    Some(m) => m.sample((x as f32 - 1.5) / 4.0, (yy as f32 - 1.5) / 4.0),
                    None => 1.0,
                };
                *o = shrink(raw, tj * s);
            }
        });
        recon_details.push(d);
        cur = blurred;
    }
    // Reconstrucción: residuo grueso + bandas de detalle ya limpias.
    let mut out = cur;
    for d in &recon_details {
        for (o, &v) in out.data.iter_mut().zip(&d.data) {
            *o += v;
        }
    }
    out
}
