//! Alineación por bloques de grueso a fino sobre pirámide gaussiana.
//!
//! Convención: el píxel (x,y) de la referencia corresponde al píxel
//! (x+dx, y+dy) de la foto alineada.
//!
//! Los píxeles saturados no tienen información de posición: pueden excluirse de la comparación
//! (`ref_max` / `alt_max`) y los bloques sin información reciben el movimiento de sus vecinos.

use crate::denoise::estimate_sigma;
use crate::develop::blur5;
use crate::image::Plane;
use crate::parallel::{par_rows, par_rows2};

#[derive(Clone, Debug)]
pub struct AlignParams {
    pub tile: usize,
    pub radius: i32,
    /// Penalización por alejarse de la predicción del nivel más grueso.
    pub lambda: f32,
    /// Los bloques de poca textura toman el movimiento de sus vecinos fiables.
    pub regularize: bool,
}

impl Default for AlignParams {
    fn default() -> Self {
        Self { tile: 16, radius: 4, lambda: 0.05, regularize: false }
    }
}

#[derive(Clone, Debug)]
pub struct Flow {
    pub tile: usize,
    pub tw: usize,
    pub th: usize,
    pub v: Vec<[f32; 2]>,
}

impl Flow {
    /// Desplazamiento interpolado bilinealmente entre centros de bloque.
    #[inline]
    pub fn at(&self, x: f32, y: f32) -> (f32, f32) {
        let t = self.tile as f32;
        let off = (t - 1.0) * 0.5;
        let gx = ((x - off) / t).clamp(0.0, (self.tw - 1) as f32);
        let gy = ((y - off) / t).clamp(0.0, (self.th - 1) as f32);
        let x0 = gx.floor() as usize;
        let y0 = gy.floor() as usize;
        let x1 = (x0 + 1).min(self.tw - 1);
        let y1 = (y0 + 1).min(self.th - 1);
        let fx = gx - x0 as f32;
        let fy = gy - y0 as f32;
        let a = self.v[y0 * self.tw + x0];
        let b = self.v[y0 * self.tw + x1];
        let c = self.v[y1 * self.tw + x0];
        let d = self.v[y1 * self.tw + x1];
        let mut out = [0.0f32; 2];
        for i in 0..2 {
            out[i] = (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy;
        }
        (out[0], out[1])
    }
}

/// Rellena los bloques sin información con el promedio de sus vecinos válidos,
/// propagando hacia el interior de la zona vacía.
fn fill_invalid(v: &mut [[f32; 2]], valid: &mut [bool], tw: usize, th: usize) {
    if valid.iter().all(|&b| b) || !valid.iter().any(|&b| b) {
        return;
    }
    loop {
        let snap_v = v.to_vec();
        let snap_ok = valid.to_vec();
        let mut changed = false;
        for ty in 0..th {
            for tx in 0..tw {
                let i = ty * tw + tx;
                if snap_ok[i] {
                    continue;
                }
                let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
                let mut add = |j: usize| {
                    if snap_ok[j] {
                        sx += snap_v[j][0];
                        sy += snap_v[j][1];
                        n += 1.0;
                    }
                };
                if tx > 0 {
                    add(i - 1);
                }
                if tx + 1 < tw {
                    add(i + 1);
                }
                if ty > 0 {
                    add(i - tw);
                }
                if ty + 1 < th {
                    add(i + tw);
                }
                if n > 0.0 {
                    v[i] = [sx / n, sy / n];
                    valid[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

fn level(r: &Plane, a: &Plane, p: &AlignParams, parent: Option<&Flow>, finest: bool, mask: (f32, f32)) -> Flow {
    let (ref_max, alt_max) = mask;
    let tile = p.tile;
    let tw = (r.w + tile - 1) / tile;
    let th = (r.h + tile - 1) / tile;
    let mut v = vec![[0.0f32; 2]; tw * th];
    let mut valid = vec![true; tw * th];
    par_rows2(&mut v, tw, &mut valid, tw, |ty, row, vrow| {
        for (tx, out) in row.iter_mut().enumerate() {
            let x0 = tx * tile;
            let y0 = ty * tile;
            let x1 = (x0 + tile).min(r.w);
            let y1 = (y0 + tile).min(r.h);
            let n = ((x1 - x0) * (y1 - y0)) as f32;

            let (ix, iy) = match parent {
                Some(f) => {
                    let q = f.v[(ty / 2).min(f.th - 1) * f.tw + (tx / 2).min(f.tw - 1)];
                    (q[0] * 2.0, q[1] * 2.0)
                }
                None => (0.0, 0.0),
            };
            let cx = ix.round() as i32;
            let cy = iy.round() as i32;

            // Suma de |diferencias| (o cuadrados) y nº de píxeles válidos (no saturados en ninguna).
            let cost = |ox: i32, oy: i32, squared: bool| -> (f32, f32) {
                let (mut c, mut cnt) = (0.0f32, 0.0f32);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let rv = r.data[y * r.w + x];
                        if rv >= ref_max {
                            continue;
                        }
                        let av = a.get(x as isize + ox as isize, y as isize + oy as isize);
                        if av >= alt_max {
                            continue;
                        }
                        let d = rv - av;
                        c += if squared { d * d } else { d.abs() };
                        cnt += 1.0;
                    }
                }
                (c, cnt)
            };

            let mut best = (f32::MAX, cx, cy);
            for dy in -p.radius..=p.radius {
                for dx in -p.radius..=p.radius {
                    let (c, cnt) = cost(cx + dx, cy + dy, false);
                    if cnt < n * 0.25 {
                        continue; // casi todo saturado: este candidato no informa
                    }
                    let s = c / cnt + p.lambda * ((dx * dx + dy * dy) as f32).sqrt();
                    if s < best.0 {
                        best = (s, cx + dx, cy + dy);
                    }
                }
            }
            if best.0 == f32::MAX {
                // Bloque sin información: se rellenará con el movimiento de sus vecinos.
                *out = [ix, iy];
                vrow[tx] = false;
                continue;
            }
            let (bx, by) = (best.1, best.2);
            let mut fx = bx as f32;
            let mut fy = by as f32;

            if finest {
                // Refinado sub-píxel: parábola sobre el coste SSD (sin sesgo de interpolación).
                let (c0, n0) = cost(bx, by, true);
                let (cl, _) = cost(bx - 1, by, true);
                let (cr, _) = cost(bx + 1, by, true);
                let (cu, _) = cost(bx, by - 1, true);
                let (cd, _) = cost(bx, by + 1, true);
                if n0 >= n * 0.25 {
                    let denx = cl - 2.0 * c0 + cr;
                    let deny = cu - 2.0 * c0 + cd;
                    if denx > 1e-6 {
                        fx += ((cl - cr) / (2.0 * denx)).clamp(-0.5, 0.5);
                    }
                    if deny > 1e-6 {
                        fy += ((cu - cd) / (2.0 * deny)).clamp(-0.5, 0.5);
                    }
                }
            }
            *out = [fx, fy];
        }
    });
    fill_invalid(&mut v, &mut valid, tw, th);
    if p.regularize {
        let mut conf = tile_confidence(r, tile, tw, th);
        for i in 0..conf.len() {
            if !valid[i] {
                conf[i] = 0.0;
            }
        }
        regularize(&mut v, &conf, tw, th);
        regularize(&mut v, &conf, tw, th);
    }
    Flow { tile, tw, th, v }
}

/// Fiabilidad (0..1) de la alineación de cada bloque: cuánta textura real tiene la referencia
/// por encima del ruido. Un bloque liso o hundido en ruido no da información de movimiento.
fn tile_confidence(r: &Plane, tile: usize, tw: usize, th: usize) -> Vec<f32> {
    let sigma = estimate_sigma(r);
    let noise_energy = (0.79 * sigma * sigma).max(1e-3);
    let blur = blur5(r);
    let mut conf = vec![0.0f32; tw * th];
    for ty in 0..th {
        for tx in 0..tw {
            let (x0, y0) = (tx * tile, ty * tile);
            let (x1, y1) = ((x0 + tile).min(r.w), (y0 + tile).min(r.h));
            let mut e = 0.0f32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let d = r.data[y * r.w + x] - blur.data[y * r.w + x];
                    e += d * d;
                }
            }
            e /= ((x1 - x0) * (y1 - y0)).max(1) as f32;
            let snr = (e - noise_energy).max(0.0) / noise_energy;
            conf[ty * tw + tx] = (snr / 2.0).clamp(0.0, 1.0);
        }
    }
    conf
}

/// El movimiento de la mano es suave: los bloques poco fiables se mezclan con el movimiento
/// de sus vecinos fiables (ventana 5x5 ponderada por fiabilidad y distancia).
fn regularize(v: &mut [[f32; 2]], conf: &[f32], tw: usize, th: usize) {
    let src = v.to_vec();
    for ty in 0..th {
        for tx in 0..tw {
            let i = ty * tw + tx;
            let c = conf[i];
            if c >= 0.999 {
                continue;
            }
            let (mut sx, mut sy, mut sw) = (0.0f32, 0.0f32, 0.0f32);
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (tx as i32 + dx, ty as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= tw as i32 || ny >= th as i32 {
                        continue;
                    }
                    let j = ny as usize * tw + nx as usize;
                    let w = conf[j] / (1.0 + 0.5 * (dx * dx + dy * dy) as f32);
                    sx += w * src[j][0];
                    sy += w * src[j][1];
                    sw += w;
                }
            }
            if sw > 1e-4 {
                let (ax, ay) = (sx / sw, sy / sw);
                v[i] = [c * src[i][0] + (1.0 - c) * ax, c * src[i][1] + (1.0 - c) * ay];
            }
        }
    }
}

/// Alinea `alt` contra `reference` (ambas pirámides, nivel 0 = resolución completa).
pub fn align(reference: &[Plane], alt: &[Plane], p: &AlignParams) -> Flow {
    align_masked(reference, alt, p, f32::MAX, f32::MAX)
}

/// Igual que `align`, pero ignora píxeles de la referencia ≥ `ref_max` y de la foto ≥ `alt_max`
/// (zonas saturadas, que no informan de la posición).
pub fn align_masked(reference: &[Plane], alt: &[Plane], p: &AlignParams, ref_max: f32, alt_max: f32) -> Flow {
    assert_eq!(reference.len(), alt.len());
    let levels = reference.len();
    let mut flow: Option<Flow> = None;
    for l in (0..levels).rev() {
        let f = level(&reference[l], &alt[l], p, flow.as_ref(), l == 0, (ref_max, alt_max));
        flow = Some(f);
    }
    flow.unwrap()
}

/// Deforma `alt` para que quede alineada con la referencia (interpolación bicúbica).
pub fn warp(alt: &Plane, flow: &Flow) -> Plane {
    let mut out = Plane::new(alt.w, alt.h);
    par_rows(&mut out.data, alt.w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let (dx, dy) = flow.at(x as f32, y as f32);
            *o = alt.sample_cubic(x as f32 + dx, y as f32 + dy);
        }
    });
    out
}
