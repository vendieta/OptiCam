//! Tubería completa: elegir referencia → alinear → fusionar (luminancia y color) → RGB.

use crate::align::{align, warp, Flow, AlignParams};
use crate::color::{denoise_chroma, half, to_rgb};
use crate::denoise::{estimate_sigma, wavelet_denoise};
use crate::develop::{nlm, restore_color, sharpen, tone_map, white_balance, DevelopParams};
use crate::pyramid::downsample;
use crate::image::Plane;
use crate::merge::{merge, merge_chroma, MergeParams};
use crate::metrics::sharpness;
use crate::noise::{estimate, estimate_temporal, NoiseModel};
use crate::package::Frame;
use crate::parallel::par_rows;
use crate::pyramid;

#[derive(Clone, Debug)]
pub struct Options {
    pub levels: usize,
    pub align: AlignParams,
    pub merge: MergeParams,
    pub develop: DevelopParams,
}

impl Default for Options {
    fn default() -> Self {
        Self { levels: 4, align: AlignParams::default(), merge: MergeParams::default(), develop: DevelopParams::default() }
    }
}

pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

pub struct Output {
    pub merged: Plane,
    pub reference_index: usize,
    pub reference: Plane,
    pub aligned: Vec<Plane>,
    pub noise: NoiseModel,
    pub white_level: f32,
    /// Foto final en color y derecha (si el paquete trae croma).
    pub rgb: Option<Rgb>,
}

/// Deforma un plano de croma (media resolución) con el flujo de luminancia.
fn warp_chroma(c: &Plane, flow: &Flow) -> Plane {
    let mut out = Plane::new(c.w, c.h);
    par_rows(&mut out.data, c.w, |j, row| {
        for (i, o) in row.iter_mut().enumerate() {
            let (dx, dy) = flow.at(2.0 * i as f32 + 0.5, 2.0 * j as f32 + 0.5);
            *o = c.sample(i as f32 + dx * 0.5, j as f32 + dy * 0.5);
        }
    });
    out
}

/// Convierte una sola foto del paquete a RGB (sin fusionar), para comparar antes/después.
pub fn frame_to_rgb(f: &Frame) -> Option<Rgb> {
    if f.cw == 0 || f.cb.len() != f.cw as usize * f.ch as usize {
        return None;
    }
    let y = Plane::from_u16(f.width as usize, f.height as usize, &f.luma);
    let cb = Plane::from_u16(f.cw as usize, f.ch as usize, &f.cb);
    let cr = Plane::from_u16(f.cw as usize, f.ch as usize, &f.cr);
    let (w, h, pixels) = to_rgb(&y, &cb, &cr, f.white_level as f32, f.rotation);
    Some(Rgb { width: w, height: h, pixels })
}

pub fn process(frames: &[Frame], opt: &Options) -> Output {
    assert!(!frames.is_empty(), "paquete vacío");
    let planes: Vec<Plane> = frames
        .iter()
        .map(|f| Plane::from_u16(f.width as usize, f.height as usize, &f.luma))
        .collect();
    let has_color = frames.iter().all(|f| f.cw > 0 && f.cb.len() == f.cw as usize * f.ch as usize);

    // 1. Referencia = la foto más nítida.
    let reference_index = (0..planes.len())
        .max_by(|&a, &b| sharpness(&planes[a]).partial_cmp(&sharpness(&planes[b])).unwrap())
        .unwrap();
    let reference = planes[reference_index].clone();
    let ref_pyr = pyramid::build(&reference, opt.levels);

    // 2. Alinear y deformar cada foto contra la referencia.
    let mut aligned = Vec::new();
    let mut flows = Vec::new();
    let mut others = Vec::new();
    for (i, p) in planes.iter().enumerate() {
        if i == reference_index {
            continue;
        }
        let pyr = pyramid::build(p, opt.levels);
        let flow = align(&ref_pyr, &pyr, &opt.align);
        aligned.push(warp(p, &flow));
        flows.push(flow);
        others.push(i);
    }

    // 3. Ruido y fusión de luminancia.
    // Ruido medido ENTRE las fotos alineadas (el que realmente se puede promediar).
    let noise = estimate_temporal(&reference, &aligned);
    let (merged, weights) = merge(&reference, &aligned, &noise, &opt.merge);
    let white = frames[0].white_level as f32;

    // 4. Color.
    let rgb = if has_color {
        let (cw, ch) = (frames[reference_index].cw as usize, frames[reference_index].ch as usize);
        let plane_of = |i: usize, v: &Vec<u16>| Plane::from_u16(frames[i].cw as usize, frames[i].ch as usize, v);
        let cb_ref = plane_of(reference_index, &frames[reference_index].cb);
        let cr_ref = plane_of(reference_index, &frames[reference_index].cr);
        let mut cb_al = Vec::new();
        let mut cr_al = Vec::new();
        for (k, &i) in others.iter().enumerate() {
            cb_al.push(warp_chroma(&plane_of(i, &frames[i].cb), &flows[k]));
            cr_al.push(warp_chroma(&plane_of(i, &frames[i].cr), &flows[k]));
        }
        let cb = merge_chroma(&cb_ref, &cb_al, &weights, merged.h);
        let cr = merge_chroma(&cr_ref, &cr_al, &weights, merged.h);
        // Ruido residual de luminancia tras fusionar N fotos.
        let sigma_ref = noise.var(128.0).sqrt();
        let sigma_m = (sigma_ref / (frames.len() as f32 * 0.6).sqrt()).max(0.3);
        // Grano: se limpia la luminancia ANTES de revelar (el revelado sube las sombras y
        // amplificaría el grano). El grano que sobrevive a la fusión (p. ej. el que deja el filtro
        // del propio teléfono) se estima directamente sobre la imagen fusionada.
        let sigma_dn = sigma_m.max(0.6 * estimate(&merged).var(100.0).sqrt());
        let merged_dn = if opt.develop.nlm > 0.0 { nlm(&merged, sigma_dn, opt.develop.nlm) } else { merged.clone() };
        // σ del ruido que queda en la imagen fusionada, medida directamente (robusta a texturas).
        let sigma_w = estimate_sigma(&merged_dn);

        let guide = half(&merged_dn, cw, ch);
        // El croma de teléfono es muy ruidoso: suavizado conjunto fuerte pero sin cruzar bordes.
        let range = (sigma_ref * 3.0).max(9.0);
        let cb = denoise_chroma(&cb, &guide, range, 2.0, 3);
        let cr = denoise_chroma(&cr, &guide, range, 2.0, 3);
        let rotation = frames[reference_index].rotation;
        let (luma_out, cb, cr) = if opt.develop.enabled {
            let (cb, cr, _tint) = if opt.develop.auto_wb { white_balance(&cb, &cr, &guide, white) } else { (cb, cr, (0.0, 0.0)) };
            let toned = tone_map(&merged_dn, white, &opt.develop, sigma_dn);
            // Cuánto cambió la luminancia en cada zona: el color debe acompañarla.
            let toned_half = half(&toned, cw, ch);
            let mut ratio = Plane::new(cw, ch);
            for i in 0..ratio.data.len() {
                ratio.data[i] = toned_half.data[i].max(1.0) / guide.data[i].max(1.0);
            }
            // El mapeo de tonos amplifica el ruido de forma distinta en cada zona (sombras más):
            // el mapa `scale` = luminancia_nueva / luminancia_vieja, a 1/4 de resolución.
            let ds_t = downsample(&downsample(&toned));
            let ds_m = downsample(&downsample(&merged_dn));
            let mut scale = Plane::new(ds_t.w, ds_t.h);
            for i in 0..scale.data.len() {
                scale.data[i] = (ds_t.data[i].max(1.0) / ds_m.data[i].max(1.0)).clamp(0.5, 3.0);
            }
            let mean_scale = scale.data.iter().sum::<f32>() / scale.data.len() as f32;
            let toned = if opt.develop.denoise > 0.0 {
                let k = 0.8 + 0.7 * opt.develop.denoise;
                wavelet_denoise(&toned, sigma_w, Some(&scale), k, 5)
            } else {
                toned
            };
            let sharp = sharpen(&toned, white, opt.develop.sharpen, sigma_w * mean_scale * 0.7);
            let (cb, cr) = restore_color(&cb, &cr, &ratio, white, opt.develop.saturation);
            (sharp, cb, cr)
        } else {
            (merged_dn.clone(), cb, cr)
        };
        let (w, h, pixels) = to_rgb(&luma_out, &cb, &cr, white, rotation);
        Some(Rgb { width: w, height: h, pixels })
    } else {
        None
    };

    Output { merged, reference_index, reference, aligned, noise, white_level: white, rgb }
}
