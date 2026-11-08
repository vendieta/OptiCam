//! Tubería completa:  elegir referencia → alinear → FUSIONAR (estándar o HDR) → REVELAR → RGB.

use crate::align::{align, align_masked, warp, AlignParams, Flow};
use crate::color::{denoise_chroma, half, to_rgb};
use crate::deconv::{estimate_blur_sigma, richardson_lucy};
use crate::denoise::{estimate_sigma, wavelet_denoise};
use crate::develop::{nlm, restore_color, sharpen, tone_map, white_balance, DevelopParams};
use crate::hdr::{estimate_gamma, exposure, is_bracketed, merge_chroma_hdr, merge_hdr, normalized, GAMMA0};
use crate::image::Plane;
use crate::merge::{merge, merge_chroma, MergeParams};
use crate::metrics::sharpness;
use crate::noise::{estimate, estimate_temporal, NoiseModel};
use crate::package::Frame;
use crate::parallel::par_rows;
use crate::pyramid::{self, downsample};
use crate::wiener::merge_wiener;

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

impl Options {
    /// Niveles de "detalle" para elegir entre suavidad y textura fina:
    ///  0 = Suave (máxima limpieza de ruido, aspecto liso)
    ///  1 = Equilibrado (por defecto)
    ///  2 = Máximo detalle (conserva la textura fina; deja algo más de grano)
    pub fn with_detail(level: u8) -> Self {
        let mut o = Options::default();
        match level {
            0 => {
                o.merge.sharp_weight = 0.0;
                o.develop.nlm = 1.0;
                o.develop.denoise = 2.5;
                o.develop.deconv = 0.3;
            }
            2 => {
                o.merge.sharp_weight = 4.0;
                o.develop.nlm = 0.1;
                o.develop.denoise = 1.0;
                o.develop.sharpen = 0.5;
                o.develop.deconv = 1.0;
            }
            _ => {}
        }
        o
    }
}

pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

pub struct Output {
    /// Luminancia fusionada (en modo HDR puede superar el blanco de la referencia).
    pub merged: Plane,
    pub reference_index: usize,
    pub reference: Plane,
    pub aligned: Vec<Plane>,
    pub noise: NoiseModel,
    pub white_level: f32,
    /// ¿Se usó la fusión HDR?
    pub hdr: bool,
    /// Gamma medida (solo HDR).
    pub gamma: f32,
    /// σ del emborronado deshecho por la deconvolución (0 si no se aplicó).
    pub psf_sigma: f32,
    /// Foto final en color y derecha (si el paquete trae croma).
    pub rgb: Option<Rgb>,
}

/// Resultado de la etapa de fusión, entrada del revelado.
struct Fused {
    merged: Plane,
    cb: Option<Plane>,
    cr: Option<Plane>,
    reference_index: usize,
    reference: Plane,
    aligned: Vec<Plane>,
    noise: NoiseModel,
    hdr: bool,
    gamma: f32,
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

fn luma_planes(frames: &[Frame]) -> Vec<Plane> {
    frames.iter().map(|f| Plane::from_u16(f.width as usize, f.height as usize, &f.luma)).collect()
}

fn chroma_plane(f: &Frame, v: &Vec<u16>) -> Plane {
    Plane::from_u16(f.cw as usize, f.ch as usize, v)
}

fn has_color(frames: &[Frame]) -> bool {
    frames.iter().all(|f| f.cw > 0 && f.cb.len() == f.cw as usize * f.ch as usize)
}

// =========================================================================================
//  Fusión estándar (todas las fotos con la misma exposición)
// =========================================================================================

fn fuse_standard(frames: &[Frame], opt: &Options) -> Fused {
    let planes = luma_planes(frames);
    let color = has_color(frames);

    // Referencia = la foto más nítida.
    let reference_index = (0..planes.len())
        .max_by(|&a, &b| sharpness(&planes[a]).partial_cmp(&sharpness(&planes[b])).unwrap())
        .unwrap();
    let reference = planes[reference_index].clone();
    let ref_pyr = pyramid::build(&reference, opt.levels);

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

    // Ruido medido ENTRE las fotos alineadas (el que realmente se puede promediar).
    let noise = estimate_temporal(&reference, &aligned);
    let (merged, weights) = if opt.merge.wiener {
        merge_wiener(&reference, &aligned, &noise, opt.merge.c)
    } else {
        merge(&reference, &aligned, &noise, &opt.merge)
    };

    let (cb, cr) = if color {
        let rf = &frames[reference_index];
        let cb_ref = chroma_plane(rf, &rf.cb);
        let cr_ref = chroma_plane(rf, &rf.cr);
        let mut cb_al = Vec::new();
        let mut cr_al = Vec::new();
        for (k, &i) in others.iter().enumerate() {
            cb_al.push(warp_chroma(&chroma_plane(&frames[i], &frames[i].cb), &flows[k]));
            cr_al.push(warp_chroma(&chroma_plane(&frames[i], &frames[i].cr), &flows[k]));
        }
        (Some(merge_chroma(&cb_ref, &cb_al, &weights, merged.h)), Some(merge_chroma(&cr_ref, &cr_al, &weights, merged.h)))
    } else {
        (None, None)
    };
    Fused { merged, cb, cr, reference_index, reference, aligned, noise, hdr: false, gamma: GAMMA0 }
}

// =========================================================================================
//  Fusión HDR (exposiciones distintas)
// =========================================================================================

fn fuse_hdr(frames: &[Frame], opt: &Options) -> Fused {
    let planes = luma_planes(frames);
    let color = has_color(frames);
    let n = frames.len();

    // Referencia: la más nítida entre las de exposición mediana.
    let mut exps: Vec<f64> = frames.iter().map(exposure).collect();
    let mut sorted = exps.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[n / 2];
    let reference_index = (0..n)
        .filter(|&i| (exps[i] / median).log2().abs() < 0.25)
        .max_by(|&a, &b| sharpness(&planes[a]).partial_cmp(&sharpness(&planes[b])).unwrap())
        .unwrap();
    let e_ref = exps[reference_index];
    for v in exps.iter_mut() {
        *v /= e_ref;
    }
    let e: Vec<f32> = exps.iter().map(|&v| v as f32).collect();
    let reference = planes[reference_index].clone();
    let ref_pyr = pyramid::build(&reference, opt.levels);

    // Alinear con las fotos llevadas al brillo de la referencia.
    let mut aligned: Vec<Plane> = Vec::new();
    let mut flows = Vec::new();
    let mut others = Vec::new();
    for (i, p) in planes.iter().enumerate() {
        if i == reference_index {
            continue;
        }
        let pyr = pyramid::build(&normalized(p, e[i], GAMMA0), opt.levels);
        // Los píxeles saturados no informan de la posición: se excluyen (en la escala de cada foto).
        let flow = align_masked(&ref_pyr, &pyr, &opt.align, 228.0, 228.0 * e[i].powf(-1.0 / GAMMA0));
        aligned.push(warp(p, &flow)); // valores originales, solo reposicionados
        flows.push(flow);
        others.push(i);
    }

    // Gamma real de la cámara y ruido (medido con las fotos de igual exposición que la referencia).
    let alts: Vec<(&Plane, f32)> = others.iter().enumerate().map(|(k, &i)| (&aligned[k], e[i])).collect();
    let gamma = estimate_gamma(&reference, &alts);
    let same: Vec<Plane> = others
        .iter()
        .enumerate()
        .filter(|(_, &i)| e[i].ln().abs() < 0.2)
        .map(|(k, _)| aligned[k].clone())
        .collect();
    let noise = if same.is_empty() {
        // Sin pares de igual exposición el estimador espacial subestima mucho el ruido real.
        let m = estimate(&reference);
        NoiseModel { a: m.a * 4.0, b: m.b * 4.0 }
    } else {
        estimate_temporal(&reference, &same)
    };

    // Todas las fotos alineadas, en su orden original (la referencia en su sitio).
    let all_luma: Vec<&Plane> = (0..n)
        .map(|i| if i == reference_index { &reference } else { &aligned[others.iter().position(|&o| o == i).unwrap()] })
        .collect();
    let (merged, weights) = merge_hdr(&all_luma, &e, reference_index, &noise, gamma, &opt.merge);

    let (cb, cr) = if color {
        let mut cbs: Vec<Plane> = Vec::new();
        let mut crs: Vec<Plane> = Vec::new();
        for i in 0..n {
            if i == reference_index {
                cbs.push(chroma_plane(&frames[i], &frames[i].cb));
                crs.push(chroma_plane(&frames[i], &frames[i].cr));
            } else {
                let k = others.iter().position(|&o| o == i).unwrap();
                cbs.push(warp_chroma(&chroma_plane(&frames[i], &frames[i].cb), &flows[k]));
                crs.push(warp_chroma(&chroma_plane(&frames[i], &frames[i].cr), &flows[k]));
            }
        }
        let cbr: Vec<&Plane> = cbs.iter().collect();
        let crr: Vec<&Plane> = crs.iter().collect();
        (
            Some(merge_chroma_hdr(&cbr, &weights, merged.h, reference_index)),
            Some(merge_chroma_hdr(&crr, &weights, merged.h, reference_index)),
        )
    } else {
        (None, None)
    };
    Fused { merged, cb, cr, reference_index, reference, aligned, noise, hdr: true, gamma }
}

// =========================================================================================
//  Revelado (común a ambos caminos)
// =========================================================================================

fn finish(frames: &[Frame], f: Fused, opt: &Options) -> Output {
    let white = frames[0].white_level as f32;
    let mut psf_used = 0.0f32;
    let rgb = match (&f.cb, &f.cr) {
        (Some(cb), Some(cr)) => {
            let rf = &frames[f.reference_index];
            let (cw, ch) = (rf.cw as usize, rf.ch as usize);
            let merged = &f.merged;
            // Ruido residual de luminancia tras fusionar N fotos.
            let sigma_ref = f.noise.var(128.0).sqrt();
            let sigma_m = (sigma_ref / (frames.len() as f32 * 0.6).sqrt()).max(0.3);
            // El ruido se limpia ANTES de revelar (el revelado sube las sombras y lo amplificaría).
            let sigma_dn = sigma_m.max(0.6 * estimate(merged).var(100.0).sqrt());
            let merged_dn = if opt.develop.nlm > 0.0 { nlm(merged, sigma_dn, opt.develop.nlm) } else { merged.clone() };
            // σ del ruido que queda, medida directamente (robusta a texturas).
            let sigma_w = estimate_sigma(&merged_dn);
            // Deshacer el emborronado óptico sobre la imagen ya limpia (no amplifica el ruido).
            let mut psf_sigma = 0.0;
            let merged_dn = if opt.develop.deconv > 0.0 {
                // σ ≤ 0 → automática: se mide en los bordes de la propia foto. Sobreestimarla
                // empeora mucho el resultado, así que se aplica un factor prudente.
                let sigma = if opt.develop.deconv_sigma > 0.0 {
                    Some(opt.develop.deconv_sigma)
                } else {
                    estimate_blur_sigma(&merged_dn).map(|s| s * opt.develop.deconv_safety)
                };
                match sigma {
                    Some(s) => {
                        psf_sigma = s;
                        richardson_lucy(&merged_dn, s, 8, opt.develop.deconv)
                    }
                    None => merged_dn,
                }
            } else {
                merged_dn
            };
            psf_used = psf_sigma;

            let guide = half(&merged_dn, cw, ch);
            // El croma de teléfono es muy ruidoso: suavizado conjunto fuerte pero sin cruzar bordes.
            let range = (sigma_ref * 3.0).max(9.0);
            let cb = denoise_chroma(cb, &guide, range, 2.0, 3);
            let cr = denoise_chroma(cr, &guide, range, 2.0, 3);
            let (luma_out, cb, cr) = if opt.develop.enabled {
                let (cb, cr, _tint) = if opt.develop.auto_wb { white_balance(&cb, &cr, &guide, white) } else { (cb, cr, (0.0, 0.0)) };
                let toned = tone_map(&merged_dn, white, &opt.develop, sigma_dn);
                // Cuánto cambió la luminancia en cada zona: el color debe acompañarla.
                let toned_half = half(&toned, cw, ch);
                let mut ratio = Plane::new(cw, ch);
                for i in 0..ratio.data.len() {
                    ratio.data[i] = toned_half.data[i].max(1.0) / guide.data[i].max(1.0);
                }
                // El mapeo de tonos amplifica el ruido de forma distinta en cada zona.
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
            let (w, h, pixels) = to_rgb(&luma_out, &cb, &cr, white, rf.rotation);
            Some(Rgb { width: w, height: h, pixels })
        }
        _ => None,
    };
    Output {
        merged: f.merged,
        reference_index: f.reference_index,
        reference: f.reference,
        aligned: f.aligned,
        noise: f.noise,
        white_level: white,
        hdr: f.hdr,
        gamma: f.gamma,
        psf_sigma: psf_used,
        rgb,
    }
}

pub fn process(frames: &[Frame], opt: &Options) -> Output {
    assert!(!frames.is_empty(), "paquete vacío");
    let fused = if frames.len() > 2 && is_bracketed(frames) { fuse_hdr(frames, opt) } else { fuse_standard(frames, opt) };
    finish(frames, fused, opt)
}
