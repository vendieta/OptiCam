//! Revelado "estilo teléfono tope de gama": mapeo de tonos local (HDR), nitidez con
//! umbral de ruido y saturación. Todo propio.

use crate::image::Plane;
use crate::parallel::par_rows;
use crate::pyramid::downsample;

#[derive(Clone, Debug)]
pub struct DevelopParams {
    /// 0 = sin cambio de tono, 1 = HDR local fuerte.
    pub strength: f32,
    pub saturation: f32,
    /// Cantidad de nitidez (0 = ninguna).
    pub sharpen: f32,
    /// Limpieza de ruido multi-escala (wavelets): 0 = ninguna, 3 = fuerte.
    pub denoise: f32,
    /// Limpieza previa por promedios no locales (0 = ninguna).
    pub nlm: f32,
    /// Corrige el tinte de la luz usando los blancos de la escena.
    pub auto_wb: bool,
    pub enabled: bool,
}

impl Default for DevelopParams {
    fn default() -> Self {
        Self { strength: 0.70, saturation: 1.25, sharpen: 0.4, denoise: 2.0, nlm: 1.0, auto_wb: true, enabled: true }
    }
}

/// Promedio con ventana cuadrada (bordes con ventana reducida).
fn box_blur(p: &Plane, r: usize) -> Plane {
    let (w, h) = (p.w, p.h);
    let mut tmp = Plane::new(w, h);
    for y in 0..h {
        let mut pre = vec![0.0f64; w + 1];
        for x in 0..w {
            pre[x + 1] = pre[x] + p.data[y * w + x] as f64;
        }
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r + 1).min(w);
            tmp.data[y * w + x] = ((pre[hi] - pre[lo]) / (hi - lo) as f64) as f32;
        }
    }
    let mut out = Plane::new(w, h);
    for x in 0..w {
        let mut pre = vec![0.0f64; h + 1];
        for y in 0..h {
            pre[y + 1] = pre[y] + tmp.data[y * w + x] as f64;
        }
        for y in 0..h {
            let lo = y.saturating_sub(r);
            let hi = (y + r + 1).min(h);
            out.data[y * w + x] = ((pre[hi] - pre[lo]) / (hi - lo) as f64) as f32;
        }
    }
    out
}

/// Desenfoque gaussiano 5x5 separable a resolución completa.
fn blur5(p: &Plane) -> Plane {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let mut tmp = Plane::new(p.w, p.h);
    par_rows(&mut tmp.data, p.w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (k, kv) in K.iter().enumerate() {
                s += kv * p.get(x as isize + k as isize - 2, y as isize);
            }
            *o = s;
        }
    });
    let mut out = Plane::new(p.w, p.h);
    par_rows(&mut out.data, p.w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (k, kv) in K.iter().enumerate() {
                s += kv * tmp.get(x as isize, y as isize + k as isize - 2);
            }
            *o = s;
        }
    });
    out
}

/// Mapeo de tonos local: separa la imagen en "base" (iluminación) y "detalle", comprime la
/// base (sombras suben, luces bajan) y conserva/realza el detalle. La base se calcula con un
/// filtro guiado que respeta los bordes (sin halos).
pub fn tone_map(y: &Plane, white: f32, p: &DevelopParams, sigma: f32) -> Plane {
    let sigma_l = sigma / white;
    const OFF: f32 = 0.02;
    let (w, h) = (y.w, y.h);
    let mut logl = Plane::new(w, h);
    par_rows(&mut logl.data, w, |yy, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let l = (y.data[yy * w + x] / white).clamp(0.0, 1.0);
            *o = (l + OFF).ln();
        }
    });

    // Filtro guiado rápido: coeficientes a,b a 1/4 de resolución, aplicados a resolución completa.
    let ds = downsample(&downsample(&logl));
    let r = 14usize.min(ds.w.min(ds.h) / 4).max(2);
    let eps = 0.03f32;
    let mean_i = box_blur(&ds, r);
    let mut ii = ds.clone();
    for v in ii.data.iter_mut() {
        *v *= *v;
    }
    let mean_ii = box_blur(&ii, r);
    let mut a = Plane::new(ds.w, ds.h);
    let mut b = Plane::new(ds.w, ds.h);
    for i in 0..ds.data.len() {
        let var = (mean_ii.data[i] - mean_i.data[i] * mean_i.data[i]).max(0.0);
        let ai = var / (var + eps);
        a.data[i] = ai;
        b.data[i] = mean_i.data[i] - ai * mean_i.data[i];
    }
    let a = box_blur(&a, r);
    let b = box_blur(&b, r);
    let mid = mean_i.data.iter().sum::<f32>() / mean_i.data.len() as f32;

    let s = p.strength.clamp(0.0, 1.0);
    // Solo se levantan las sombras; las luces NO se comprimen (el blanco debe seguir blanco).
    let comp = 1.0 - 0.55 * s;
    let boost = 1.0 + 0.30 * s;

    let mut out = Plane::new(w, h);
    par_rows(&mut out.data, w, |yy, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let fx = (x as f32 - 1.5) / 4.0;
            let fy = (yy as f32 - 1.5) / 4.0;
            let i = logl.data[yy * w + x];
            let base = a.sample(fx, fy) * i + b.sample(fx, fy);
            let detail = i - base;
            // El realce de detalle solo actúa sobre lo que supera el ruido: d(ln L) = σ / L.
            let noise_log = sigma_l * (-i).exp();
            let core = detail.signum() * (detail.abs() - 2.5 * noise_log).max(0.0);
            let base_t = if base < mid { mid + (base - mid) * comp } else { base };
            let ol = base_t + detail + (boost - 1.0) * core;
            *o = ol.exp() - OFF;
        }
    });

    // Normalización de brillo: escenas oscuras suben (sin pasarse).
    let step = (w * h / 40_000).max(1);
    let (mut sum, mut n) = (0.0f64, 0u32);
    let mut i = 0;
    while i < out.data.len() {
        sum += out.data[i].clamp(0.0, 1.0) as f64;
        n += 1;
        i += step;
    }
    let mean = (sum / n.max(1) as f64) as f32;
    let gain = (0.32 / mean.max(0.02)).clamp(1.0, 1.6).powf(s);

    par_rows(&mut out.data, w, |_, row| {
        for v in row.iter_mut() {
            let mut x = (*v * gain).clamp(0.0, 1.5);
            // Curva en S suave: devuelve "pegada" (sombras más profundas, luces más claras)
            // que la compresión local le quita a la imagen.
            if x < 1.0 {
                let ss = x * x * (3.0 - 2.0 * x);
                x += (ss - x) * 0.40 * s;
            }
            // Hombro suave para las luces.
            if x > 0.80 {
                x = 0.80 + 0.20 * ((x - 0.80) / 0.20).tanh();
            }
            *v = x * white;
        }
    });
    out
}

/// Máscara de nitidez con umbral (coring): solo realza detalle por encima del ruido.
pub fn sharpen(y: &Plane, white: f32, amount: f32, sigma: f32) -> Plane {
    let blur = blur5(y);
    let thr = (sigma * 2.2).max(1.0) * white / 255.0;
    let mut out = Plane::new(y.w, y.h);
    par_rows(&mut out.data, y.w, |yy, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = yy * y.w + x;
            let d = y.data[i] - blur.data[i];
            let m = (d.abs() - thr).max(0.0);
            let boosted = y.data[i] + amount * d.signum() * m;
            *o = boosted.clamp(0.0, white);
        }
    });
    out
}

/// Balance de blancos automático: busca píxeles claros y casi neutros (blancos y grises
/// luminosos), mide su tinte medio y lo resta en proporción al brillo. Si no hay suficientes
/// píxeles neutros (p. ej. foto de una pared roja) no toca nada. Devuelve (cb, cr, tinte).
pub fn white_balance(cb: &Plane, cr: &Plane, luma_half: &Plane, white: f32) -> (Plane, Plane, (f32, f32)) {
    let mid = (white + 1.0) / 2.0;
    let k = 255.0 / white;
    let mut sel_cb = Vec::new();
    let mut sel_cr = Vec::new();
    let mut sel_y = Vec::new();
    for i in 0..cb.data.len() {
        let y = luma_half.data[i] * k;
        let dcb = (cb.data[i] - mid) * k;
        let dcr = (cr.data[i] - mid) * k;
        // Claros (> 55 % del rango), no saturados y con poco tinte.
        if y > 140.0 && y < 250.0 && dcb.abs() < 18.0 && dcr.abs() < 18.0 {
            sel_cb.push(dcb);
            sel_cr.push(dcr);
            sel_y.push(y);
        }
    }
    let n = sel_cb.len();
    if n < cb.data.len() / 200 || n < 200 {
        return (cb.clone(), cr.clone(), (0.0, 0.0));
    }
    let med = |v: &mut Vec<f32>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let (mcb, mcr, my) = (med(&mut sel_cb), med(&mut sel_cr), med(&mut sel_y));
    let strength = 0.9;
    let mut ocb = cb.clone();
    let mut ocr = cr.clone();
    for i in 0..ocb.data.len() {
        // El tinte de la luz crece con el brillo: negros siguen negros.
        let f = ((luma_half.data[i] * k) / my.max(1.0)).clamp(0.0, 1.2) * strength / k;
        ocb.data[i] = (cb.data[i] - mcb * f).clamp(0.0, white);
        ocr.data[i] = (cr.data[i] - mcr * f).clamp(0.0, white);
    }
    (ocb, ocr, (mcb, mcr))
}

/// Restaura y realza el color tras el mapeo de tonos.
///
/// Al cambiar la luminancia por un factor `r`, escalar el croma por ~r mantiene las
/// proporciones de color (sin esto la imagen queda "lavada"). Además aplica "vibrance":
/// realza los colores moderados, deja intactos los casi neutros (blancos y grises no se
/// tiñen) y frena los ya muy intensos.
/// `ratio` = luminancia_nueva / luminancia_vieja, a la resolución del croma.
pub fn restore_color(cb: &Plane, cr: &Plane, ratio: &Plane, white: f32, sat: f32) -> (Plane, Plane) {
    let mid = (white + 1.0) / 2.0;
    let k = 255.0 / white;
    let mut ocb = cb.clone();
    let mut ocr = cr.clone();
    for i in 0..ocb.data.len() {
        let (dcb, dcr) = (cb.data[i] - mid, cr.data[i] - mid);
        let mag = (dcb * dcb + dcr * dcr).sqrt() * k;
        let keep = ratio.data[i].clamp(0.6, 2.5).powf(0.9);
        let rise = ((mag - 5.0) / 20.0).clamp(0.0, 1.0);
        let fall = 1.0 - 0.5 * ((mag - 45.0) / 60.0).clamp(0.0, 1.0);
        let vib = 1.0 + (sat - 1.0) * rise * fall;
        ocb.data[i] = (mid + dcb * keep * vib).clamp(0.0, white);
        ocr.data[i] = (mid + dcr * keep * vib).clamp(0.0, white);
    }
    (ocb, ocr)
}

/// Reducción de grano de luminancia por promedios no locales (NLM): cada píxel se promedia
/// con los de su entorno cuyo parche 3x3 se le parece. Suaviza el grano y conserva los bordes.
pub fn nlm(y: &Plane, sigma: f32, strength: f32) -> Plane {
    const R: isize = 3; // ventana de búsqueda 7x7
    let (w, h) = (y.w, y.h);
    let sig2 = sigma * sigma;
    let h2 = (strength * sigma).max(0.3).powi(2);
    let mut out = Plane::new(w, h);
    par_rows(&mut out.data, w, |yy, row| {
        let yi = yy as isize;
        for (x, o) in row.iter_mut().enumerate() {
            let xi = x as isize;
            let mut patch = [0.0f32; 9];
            for (n, (dy, dx)) in [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 0), (0, 1), (1, -1), (1, 0), (1, 1)].iter().enumerate() {
                patch[n] = y.get(xi + dx, yi + dy);
            }
            let mut acc = 0.0f32;
            let mut wsum = 0.0f32;
            let mut wmax = 0.0f32;
            for sy in -R..=R {
                for sx in -R..=R {
                    if sx == 0 && sy == 0 {
                        continue;
                    }
                    let mut d2 = 0.0;
                    for (n, (dy, dx)) in [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 0), (0, 1), (1, -1), (1, 0), (1, 1)].iter().enumerate() {
                        let d = y.get(xi + sx + dx, yi + sy + dy) - patch[n];
                        d2 += d * d;
                    }
                    d2 /= 9.0;
                    let wgt = (-((d2 - 2.0 * sig2).max(0.0)) / h2).exp();
                    acc += wgt * y.get(xi + sx, yi + sy);
                    wsum += wgt;
                    wmax = wmax.max(wgt);
                }
            }
            let wc = wmax.max(1e-3);
            *o = (acc + wc * patch[4]) / (wsum + wc);
        }
    });
    out
}
