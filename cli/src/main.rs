mod server;
mod synth;

use camara_core::image::Plane;
use camara_core::metrics::psnr;
use camara_core::package::{read_package, write_package};
use camara_core::pipeline::{process, Options};
use camara_core::png::{write_gray, write_rgb};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::time::Instant;

fn save_png(path: &str, p: &Plane, white: f32) {
    let mut f = BufWriter::new(File::create(path).expect("no se pudo crear el PNG"));
    write_gray(&mut f, p.w, p.h, &p.to_u8(white)).expect("error escribiendo PNG");
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn bench(args: &[String]) {
    let n: usize = arg(args, "--frames", 8);
    let w: usize = arg(args, "--w", 640);
    let h: usize = arg(args, "--h", 480);
    let na: f32 = arg(args, "--noise-a", 0.8);
    let nb: f32 = arg(args, "--noise-b", 9.0);
    std::fs::create_dir_all("out").ok();

    println!("Banco sintético: {n} fotos de {w}x{h}, ruido var = {na}·v + {nb}");
    let scene = synth::Scene::new(w, h, 7);
    let rot: f32 = arg(args, "--rot", 0.0);
    let blur: f32 = arg(args, "--blur", 0.0);
    let lens: f32 = arg(args, "--lens", 0.0);
    let burst = synth::make_burst(&scene, n, na, nb, 42, rot, blur, lens);

    let mut opt = Options::default();
    opt.merge.wiener = arg(args, "--wiener", opt.merge.wiener as u8) != 0;
    opt.merge.c = arg(args, "--c", opt.merge.c);
    opt.merge.t0 = arg(args, "--t0", opt.merge.t0);
    opt.merge.sharp_weight = arg(args, "--sharpw", opt.merge.sharp_weight);
    opt.align.tile = arg(args, "--tile", opt.align.tile);
    opt.align.regularize = arg(args, "--reg", opt.align.regularize as u8) != 0;
    opt.align.radius = arg(args, "--radius", opt.align.radius);
    opt.levels = arg(args, "--levels", opt.levels);
    let t0 = Instant::now();
    let out = process(&burst.frames, &opt);
    let dt = t0.elapsed();

    let r = out.reference_index;
    let (sx, sy) = burst.shifts[r];
    let truth = scene.render_rot(sx, sy, burst.rots[r], r);
    let border = 24;
    // Retención de detalle fino: ganancia de proyección sobre la verdad (1.0 = detalle intacto,
    // <1 = emborronado; el ruido no sesga esta medida).
    let gain = |est: &Plane, reg: (f32, f32, f32, f32)| -> f32 {
        let (x0, y0, x1, y1) = ((reg.0 * w as f32) as usize + 6, (reg.1 * h as f32) as usize + 6, (reg.2 * w as f32) as usize - 6, (reg.3 * h as f32) as usize - 6);
        let (mut me, mut mt, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in y0..y1 {
            for x in x0..x1 {
                me += est.data[y * w + x] as f64;
                mt += truth.data[y * w + x] as f64;
                n += 1.0;
            }
        }
        let (me, mt) = (me / n, mt / n);
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for y in y0..y1 {
            for x in x0..x1 {
                let (e, t) = (est.data[y * w + x] as f64 - me, truth.data[y * w + x] as f64 - mt);
                num += e * t;
                den += t * t;
            }
        }
        (num / den.max(1e-9)) as f32
    };
    let grating = (0.05f32, 0.62f32, 0.30f32, 0.95f32);
    let bars = (0.35f32, 0.62f32, 0.55f32, 0.90f32);
    let texture = synth::TEXTURE;

    // Referencias de comparación.
    let mut naive = Plane::new(w, h);
    for f in &burst.frames {
        for (o, &v) in naive.data.iter_mut().zip(&f.luma) {
            *o += v as f32 / n as f32;
        }
    }
    let mut avg_aligned = out.reference.clone();
    for a in &out.aligned {
        for (o, &v) in avg_aligned.data.iter_mut().zip(&a.data) {
            *o += v;
        }
    }
    for o in avg_aligned.data.iter_mut() {
        *o /= n as f32;
    }

    println!("Referencia elegida: foto {r}");
    println!(
        "Ruido estimado: var = {:.3}·v + {:.3}  (real: {na}·v + {nb})",
        out.noise.a, out.noise.b
    );
    println!("Tiempo del motor: {:.2?}", dt);
    println!(
        "DETALLE FINO conservado (1.00 = intacto):  rejilla fina  foto única {:.2} → motor {:.2} | barras  foto única {:.2} → motor {:.2}",
        gain(&out.reference, grating), gain(&out.merged, grating), gain(&out.reference, bars), gain(&out.merged, bars)
    );
    {
        // ¿Cuánto detalle fino quitan los filtros de limpieza de ruido del revelado?
        use camara_core::denoise::{estimate_sigma, wavelet_denoise};
        use camara_core::develop::nlm;
        let sigma_ref = out.noise.var(128.0).sqrt();
        let sigma_m = (sigma_ref / (n as f32 * 0.6).sqrt()).max(0.3);
        let sigma_dn = sigma_m.max(0.6 * camara_core::noise::estimate(&out.merged).var(100.0).sqrt());
        let d = Options::default().develop;
        let m1 = nlm(&out.merged, sigma_dn, d.nlm);
        let sigma_w = estimate_sigma(&m1);
        let m2 = wavelet_denoise(&m1, sigma_w, None, 0.8 + 0.7 * d.denoise, 5);
        println!(
            "  Tras limpiar ruido (σ_nlm={sigma_dn:.2}, σ_wav={sigma_w:.2}):  rejilla  fusión {:.2} → NLM {:.2} → +wavelet {:.2} | barras  {:.2} → {:.2} → {:.2}   (PSNR {:.2} → {:.2} → {:.2} dB)",
            gain(&out.merged, grating), gain(&m1, grating), gain(&m2, grating),
            gain(&out.merged, bars), gain(&m1, bars), gain(&m2, bars),
            psnr(&out.merged, &truth, border, 255.0), psnr(&m1, &truth, border, 255.0), psnr(&m2, &truth, border, 255.0)
        );
        println!(
            "  TEXTURA FINA DE BAJO CONTRASTE:  foto única {:.2} | fusión {:.2} → NLM {:.2} → +wavelet {:.2}",
            gain(&out.reference, texture), gain(&out.merged, texture), gain(&m1, texture), gain(&m2, texture)
        );
        if args.iter().any(|a| a == "--sweep-deconv") {
            println!("    σ de emborronado MEDIDA en los bordes: {:?}  (σ real del lente = {lens})", camara_core::deconv::estimate_blur_sigma(&m2));
            println!("    Sin deconvolución:        rejilla {:.2} | barras {:.2} | textura {:.2} | PSNR {:.2} dB", gain(&m2, grating), gain(&m2, bars), gain(&m2, texture), psnr(&m2, &truth, border, 255.0));
            for ds in [0.5f32, 0.7, 0.9, 1.1] {
                for st in [0.6f32, 1.0] {
                    let m = camara_core::deconv::richardson_lucy(&m2, ds, 8, st);
                    println!(
                        "    RL σ={ds:.1} fuerza {st:.1}:    rejilla {:.2} | barras {:.2} | textura {:.2} | PSNR {:.2} dB",
                        gain(&m, grating), gain(&m, bars), gain(&m, texture), psnr(&m, &truth, border, 255.0)
                    );
                }
            }
        }
        if args.iter().any(|a| a == "--sweep-nlm") {
            for s in [0.0f32, 0.15, 0.3, 0.6, 1.0] {
                let m = if s > 0.0 { nlm(&out.merged, sigma_dn, s) } else { out.merged.clone() };
                println!(
                    "    NLM fuerza {s:.2}:  textura {:.2} | rejilla {:.2} | PSNR {:.2} dB",
                    gain(&m, texture), gain(&m, grating), psnr(&m, &truth, border, 255.0)
                );
            }
        }
    }
    println!("PSNR frente a la verdad (más alto = mejor):");
    println!("  Foto única (referencia):          {:.2} dB", psnr(&out.reference, &truth, border, 255.0));
    println!("  Promedio sin alinear:             {:.2} dB", psnr(&naive, &truth, border, 255.0));
    println!("  Promedio alineado (sin robustez): {:.2} dB", psnr(&avg_aligned, &truth, border, 255.0));
    println!("  Motor completo:                   {:.2} dB", psnr(&out.merged, &truth, border, 255.0));

    save_png("out/referencia.png", &out.reference, 255.0);
    save_png("out/fusionada.png", &out.merged, 255.0);
    save_png("out/verdad.png", &truth, 255.0);
    save_png("out/promedio_alineado.png", &avg_aligned, 255.0);
    if let Some(rgb) = &out.rgb {
        let mut f = BufWriter::new(File::create("out/fusionada_color.png").unwrap());
        write_rgb(&mut f, rgb.width, rgb.height, &rgb.pixels).unwrap();
        let single = camara_core::pipeline::frame_to_rgb(&burst.frames[r]).unwrap();
        let mut f = BufWriter::new(File::create("out/referencia_color.png").unwrap());
        write_rgb(&mut f, single.width, single.height, &single.pixels).unwrap();
    }
    println!("PNG guardados en la carpeta out/");
}

/// Error medio en pasos de exposición (|log2| entre radiancia recuperada y real) dentro de una región.
fn log_err(est: &Plane, truth: &Plane, gamma: f32, reg: (f32, f32, f32, f32)) -> f32 {
    let (w, h) = (est.w as f32, est.h as f32);
    let (x0, y0, x1, y1) = ((reg.0 * w) as usize + 8, (reg.1 * h) as usize + 8, (reg.2 * w) as usize - 8, (reg.3 * h) as usize - 8);
    let (mut s, mut n) = (0.0f64, 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let e = ((est.data[y * est.w + x] / 255.0).max(1e-4)).powf(gamma);
            let t = truth.data[y * truth.w + x].max(1e-4);
            s += (e / t).log2().abs() as f64;
            n += 1;
        }
    }
    (s / n.max(1) as f64) as f32
}

fn bench_hdr(args: &[String]) {
    let n: usize = arg(args, "--frames", 12);
    let (w, h) = (640usize, 480usize);
    std::fs::create_dir_all("out").ok();
    let scene = synth::Scene::new(w, h, 7);
    let exposures = [1.0f32, 0.25, 2.0];
    println!("Banco HDR sintético: {n} fotos, exposiciones relativas {exposures:?} (referencia = 1.0)");
    let burst = synth::make_hdr_burst(&scene, n, &exposures, 0.8, 9.0, 42);

    let t0 = Instant::now();
    let out = process(&burst.frames, &Options::default());
    println!("Motor: {:.2?}  | HDR detectado: {}  | gamma medida: {:.2} (real: 2.20)", t0.elapsed(), out.hdr, out.gamma);

    let r = out.reference_index;
    let (sx, sy) = burst.shifts[r];
    let truth = scene.render_rad(sx, sy, r);
    println!("Referencia elegida: foto {r} (exposición {:.2})", exposures[r % exposures.len()]);
    println!("Error de radiancia en pasos de exposición (0 = perfecto; menos es mejor):");
    println!("  {:28} {:>10} {:>10}", "", "Foto única", "HDR");
    for (name, reg) in [("Lámpara (luces quemadas)", synth::LAMP), ("Zona oscura (sombras)", synth::SHADOW)] {
        println!(
            "  {:28} {:>10.2} {:>10.2}",
            name,
            log_err(&out.reference, &truth, 2.2, reg),
            log_err(&out.merged, &truth, out.gamma, reg)
        );
    }
    // Diagnóstico: valores medios en la lámpara.
    {
        let (x0, y0, x1, y1) = ((synth::LAMP.0 * w as f32) as usize + 8, (synth::LAMP.1 * h as f32) as usize + 8, (synth::LAMP.2 * w as f32) as usize - 8, (synth::LAMP.3 * h as f32) as usize - 8);
        let (mut te, mut tt, mut tv, mut n) = (0.0f64, 0.0f64, 0.0f64, 0u32);
        for y in y0..y1 {
            for x in x0..x1 {
                te += ((out.merged.data[y * w + x] / 255.0).powf(out.gamma)) as f64;
                tt += truth.data[y * w + x] as f64;
                tv += out.merged.data[y * w + x] as f64;
                n += 1;
            }
        }
        println!("  [diag lámpara] radiancia media estimada {:.2}, real {:.2}, v medio fusionado {:.1}", te / n as f64, tt / n as f64, tv / n as f64);
        for (i, f) in burst.frames.iter().enumerate().take(3) {
            let m: f64 = (y0..y1).map(|y| (x0..x1).map(|x| f.luma[y * w + x] as f64).sum::<f64>()).sum::<f64>() / n as f64;
            println!("  [diag] foto {i} (exposición {:.2}): v medio en lámpara = {m:.1}", exposures[i % 3]);
        }
    }
    if let Some(rgb) = &out.rgb {
        let mut f = BufWriter::new(File::create("out/hdr_final.png").unwrap());
        write_rgb(&mut f, rgb.width, rgb.height, &rgb.pixels).unwrap();
        let single = camara_core::pipeline::frame_to_rgb(&burst.frames[r]).unwrap();
        let mut f = BufWriter::new(File::create("out/hdr_unica.png").unwrap());
        write_rgb(&mut f, single.width, single.height, &single.pixels).unwrap();
    }
    save_png("out/hdr_fusion_cruda.png", &out.merged, 500.0);
    println!("PNG: out/hdr_unica.png (foto única) y out/hdr_final.png (HDR revelado)");
}

fn synth_cmd(args: &[String]) {
    let path = args.get(0).expect("uso: camara synth <archivo.camb>");
    let scene = synth::Scene::new(640, 480, 7);
    let burst = synth::make_burst(&scene, arg(args, "--frames", 8), 0.8, 9.0, 42, 0.0, 0.0, 0.0);
    let mut f = BufWriter::new(File::create(path).unwrap());
    write_package(&mut f, &burst.frames).unwrap();
    println!("Paquete escrito: {path}");
}

fn process_cmd(args: &[String]) {
    let (inp, outp) = match (args.get(0), args.get(1)) {
        (Some(a), Some(b)) => (a, b),
        _ => panic!("uso: camara process <entrada.camb> <salida.png>"),
    };
    let frames = read_package(&mut BufReader::new(File::open(inp).expect("no se pudo abrir"))).unwrap();
    let mut frames = frames;
    // --max-frames N: usa solo N fotos repartidas por toda la ráfaga (para estudiar el efecto del tamaño del stack).
    let max_frames: usize = arg(args, "--max-frames", frames.len());
    if max_frames > 0 && max_frames < frames.len() {
        let total = frames.len();
        let keep: Vec<usize> = (0..max_frames).map(|i| i * total / max_frames).collect();
        frames = frames.into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, f)| f).collect();
    }
    let f0 = &frames[0];
    println!(
        "Paquete: {} fotos de {}x{}, ISO {}, exposición {:.1} ms, rotación {}°",
        frames.len(), f0.width, f0.height, f0.iso, f0.exposure_ns as f64 / 1e6, f0.rotation
    );
    let t0 = Instant::now();
    let mut opt = Options::with_detail(arg(args, "--detail", 1u8));
    opt.develop.strength = arg(args, "--strength", opt.develop.strength);
    opt.develop.sharpen = arg(args, "--sharpen", opt.develop.sharpen);
    opt.develop.saturation = arg(args, "--sat", opt.develop.saturation);
    opt.develop.denoise = arg(args, "--denoise", opt.develop.denoise);
    opt.develop.nlm = arg(args, "--nlm", opt.develop.nlm);
    opt.develop.clarity = arg(args, "--clarity", opt.develop.clarity);
    opt.develop.deconv = arg(args, "--deconv", opt.develop.deconv);
    opt.develop.deconv_sigma = arg(args, "--dsigma", opt.develop.deconv_sigma);
    opt.develop.deconv_safety = arg(args, "--dsafety", opt.develop.deconv_safety);
    opt.merge.t0 = arg(args, "--t0", opt.merge.t0);
    opt.merge.wiener = arg(args, "--wiener", opt.merge.wiener as u8) != 0;
    opt.merge.c = arg(args, "--c", opt.merge.c);
    opt.merge.scale = arg(args, "--mscale", opt.merge.scale);
    opt.merge.sharp_weight = arg(args, "--sharpw", opt.merge.sharp_weight);
    opt.align.tile = arg(args, "--tile", opt.align.tile);
    opt.align.radius = arg(args, "--radius", opt.align.radius);
    opt.levels = arg(args, "--levels", opt.levels);
    opt.align.regularize = arg(args, "--reg", opt.align.regularize as u8) != 0;
    opt.develop.auto_wb = !args.iter().any(|a| a == "--no-wb");
    opt.develop.enabled = !args.iter().any(|a| a == "--raw");
    let out = process(&frames, &opt);
    println!("{} fotos procesadas en {:.2?} (referencia: {})", frames.len(), t0.elapsed(), out.reference_index);
    println!("Ruido estimado: var = {:.3}·v + {:.3}", out.noise.a, out.noise.b);
    if out.psf_sigma > 0.0 {
        println!("Deconvolución aplicada con σ = {:.2} px", out.psf_sigma);
    }
    if let Some(i) = args.iter().position(|a| a == "--single") {
        if let (Some(path), Some(s)) = (args.get(i + 1), camara_core::pipeline::frame_to_rgb(&frames[out.reference_index])) {
            let mut f = BufWriter::new(File::create(path).expect("no se pudo crear el PNG"));
            write_rgb(&mut f, s.width, s.height, &s.pixels).expect("error escribiendo PNG");
        }
    }
    match &out.rgb {
        Some(rgb) => {
            let mut f = BufWriter::new(File::create(outp).expect("no se pudo crear el PNG"));
            write_rgb(&mut f, rgb.width, rgb.height, &rgb.pixels).expect("error escribiendo PNG");
        }
        None => save_png(outp, &out.merged, out.white_level),
    }
}

/// Nitidez de cada foto de un paquete (para ver cuántas salen movidas por el temblor).
fn frames_info(args: &[String]) {
    let path = args.first().expect("uso: camara frames-info <paquete.camb>");
    let frames = read_package(&mut BufReader::new(File::open(path).expect("no se pudo abrir"))).unwrap();
    let s: Vec<f32> = frames
        .iter()
        .map(|f| camara_core::metrics::sharpness(&Plane::from_u16(f.width as usize, f.height as usize, &f.luma)))
        .collect();
    let max = s.iter().cloned().fold(0.0f32, f32::max);
    println!("{path}");
    for (i, v) in s.iter().enumerate() {
        println!("  foto {:2}: nitidez relativa {:5.1}%  {}", i + 1, v / max * 100.0, "█".repeat((v / max * 30.0) as usize));
    }
    let mean = s.iter().sum::<f32>() / s.len() as f32;
    println!("  mínima {:.1}% · media {:.1}% de la mejor", s.iter().cloned().fold(f32::MAX, f32::min) / max * 100.0, mean / max * 100.0);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("bench") => bench(&args[1..]),
        Some("bench-hdr") => bench_hdr(&args[1..]),
        Some("frames-info") => frames_info(&args[1..]),
        Some("synth") => synth_cmd(&args[1..]),
        Some("process") => process_cmd(&args[1..]),
        Some("serve") => server::serve(arg(&args[1..], "--port", 8080u16)),
        _ => println!("Comandos:\n  camara serve [--port 8080]          dashboard web en http://127.0.0.1:8080\n  camara process <entrada.camb> <salida.png>\n  camara bench [--frames N] [--w W --h H] [--noise-a A --noise-b B]\n  camara synth <archivo.camb>"),
    }
}
