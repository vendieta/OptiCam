//! Alineación por bloques de grueso a fino sobre pirámide gaussiana.
//!
//! Convención: el píxel (x,y) de la referencia corresponde al píxel
//! (x+dx, y+dy) de la foto alineada.

use crate::image::Plane;
use crate::parallel::par_rows;

#[derive(Clone, Debug)]
pub struct AlignParams {
    pub tile: usize,
    pub radius: i32,
    /// Penalización por alejarse de la predicción del nivel más grueso.
    pub lambda: f32,
}

impl Default for AlignParams {
    fn default() -> Self {
        Self { tile: 16, radius: 4, lambda: 0.05 }
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

fn level(r: &Plane, a: &Plane, p: &AlignParams, parent: Option<&Flow>, finest: bool) -> Flow {
    let tile = p.tile;
    let tw = (r.w + tile - 1) / tile;
    let th = (r.h + tile - 1) / tile;
    let mut v = vec![[0.0f32; 2]; tw * th];
    par_rows(&mut v, tw, |ty, row| {
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

            let sad = |ox: i32, oy: i32| -> f32 {
                let mut c = 0.0;
                for y in y0..y1 {
                    for x in x0..x1 {
                        c += (r.data[y * r.w + x] - a.get(x as isize + ox as isize, y as isize + oy as isize)).abs();
                    }
                }
                c
            };
            let ssd = |ox: i32, oy: i32| -> f32 {
                let mut c = 0.0;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let d = r.data[y * r.w + x] - a.get(x as isize + ox as isize, y as isize + oy as isize);
                        c += d * d;
                    }
                }
                c
            };

            let mut best = (f32::MAX, cx, cy);
            for dy in -p.radius..=p.radius {
                for dx in -p.radius..=p.radius {
                    let c = sad(cx + dx, cy + dy) / n + p.lambda * ((dx * dx + dy * dy) as f32).sqrt();
                    if c < best.0 {
                        best = (c, cx + dx, cy + dy);
                    }
                }
            }
            let (bx, by) = (best.1, best.2);
            let mut fx = bx as f32;
            let mut fy = by as f32;

            if finest {
                // Refinado sub-píxel: parábola sobre el coste SSD (sin sesgo de interpolación).
                let c0 = ssd(bx, by);
                let (cl, cr) = (ssd(bx - 1, by), ssd(bx + 1, by));
                let (cu, cd) = (ssd(bx, by - 1), ssd(bx, by + 1));
                let denx = cl - 2.0 * c0 + cr;
                let deny = cu - 2.0 * c0 + cd;
                if denx > 1e-6 {
                    fx += ((cl - cr) / (2.0 * denx)).clamp(-0.5, 0.5);
                }
                if deny > 1e-6 {
                    fy += ((cu - cd) / (2.0 * deny)).clamp(-0.5, 0.5);
                }
            }
            *out = [fx, fy];
        }
    });
    Flow { tile, tw, th, v }
}

/// Alinea `alt` contra `reference` (ambas pirámides, nivel 0 = resolución completa).
pub fn align(reference: &[Plane], alt: &[Plane], p: &AlignParams) -> Flow {
    assert_eq!(reference.len(), alt.len());
    let levels = reference.len();
    let mut flow: Option<Flow> = None;
    for l in (0..levels).rev() {
        let f = level(&reference[l], &alt[l], p, flow.as_ref(), l == 0);
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
