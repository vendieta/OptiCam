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
    let burst = synth::make_burst(&scene, n, na, nb, 42);

    let t0 = Instant::now();
    let out = process(&burst.frames, &Options::default());
    let dt = t0.elapsed();

    let r = out.reference_index;
    let (sx, sy) = burst.shifts[r];
    let truth = scene.render(sx, sy, r);
    let border = 24;

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

fn synth_cmd(args: &[String]) {
    let path = args.get(0).expect("uso: camara synth <archivo.camb>");
    let scene = synth::Scene::new(640, 480, 7);
    let burst = synth::make_burst(&scene, arg(args, "--frames", 8), 0.8, 9.0, 42);
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
    let f0 = &frames[0];
    println!(
        "Paquete: {} fotos de {}x{}, ISO {}, exposición {:.1} ms, rotación {}°",
        frames.len(), f0.width, f0.height, f0.iso, f0.exposure_ns as f64 / 1e6, f0.rotation
    );
    let t0 = Instant::now();
    let mut opt = Options::default();
    opt.develop.strength = arg(args, "--strength", opt.develop.strength);
    opt.develop.sharpen = arg(args, "--sharpen", opt.develop.sharpen);
    opt.develop.saturation = arg(args, "--sat", opt.develop.saturation);
    opt.develop.denoise = arg(args, "--denoise", opt.develop.denoise);
    opt.develop.nlm = arg(args, "--nlm", opt.develop.nlm);
    opt.merge.t0 = arg(args, "--t0", opt.merge.t0);
    opt.merge.scale = arg(args, "--mscale", opt.merge.scale);
    opt.develop.auto_wb = !args.iter().any(|a| a == "--no-wb");
    opt.develop.enabled = !args.iter().any(|a| a == "--raw");
    let out = process(&frames, &opt);
    println!("{} fotos procesadas en {:.2?} (referencia: {})", frames.len(), t0.elapsed(), out.reference_index);
    println!("Ruido estimado: var = {:.3}·v + {:.3}", out.noise.a, out.noise.b);
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("bench") => bench(&args[1..]),
        Some("synth") => synth_cmd(&args[1..]),
        Some("process") => process_cmd(&args[1..]),
        Some("serve") => server::serve(arg(&args[1..], "--port", 8080u16)),
        _ => println!("Comandos:\n  camara serve [--port 8080]          dashboard web en http://127.0.0.1:8080\n  camara process <entrada.camb> <salida.png>\n  camara bench [--frames N] [--w W --h H] [--noise-a A --noise-b B]\n  camara synth <archivo.camb>"),
    }
}
