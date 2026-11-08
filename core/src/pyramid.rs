//! Pirámide gaussiana.

use crate::image::Plane;
use crate::parallel::par_rows;

const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// Reduce a la mitad con filtro gaussiano 5x5 separable.
pub fn downsample(p: &Plane) -> Plane {
    let nw = (p.w / 2).max(1);
    let nh = (p.h / 2).max(1);
    // Paso horizontal solo en las columnas pares necesarias: tmp tiene ancho nw y alto h.
    let mut tmp = Plane::new(nw, p.h);
    par_rows(&mut tmp.data, nw, |y, row| {
        for (x, out) in row.iter_mut().enumerate() {
            let cx = (x * 2) as isize;
            let mut s = 0.0;
            for (k, &kv) in K.iter().enumerate() {
                s += kv * p.get(cx + k as isize - 2, y as isize);
            }
            *out = s;
        }
    });
    let mut out = Plane::new(nw, nh);
    par_rows(&mut out.data, nw, |y, row| {
        let cy = (y * 2) as isize;
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (k, &kv) in K.iter().enumerate() {
                s += kv * tmp.get(x as isize, cy + k as isize - 2);
            }
            *o = s;
        }
    });
    out
}

/// Nivel 0 = resolución completa.
pub fn build(p: &Plane, levels: usize) -> Vec<Plane> {
    let mut v = vec![p.clone()];
    for _ in 1..levels {
        let next = downsample(v.last().unwrap());
        v.push(next);
    }
    v
}
