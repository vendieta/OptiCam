//! Servidor web local del dashboard (solo biblioteca estándar).
//!
//! GET  /                   → página del dashboard
//! POST /process            → cuerpo = paquete .camb; responde JSON con estadísticas e id
//! GET  /img/<id>/<nombre>  → merged.png | single.png

use camara_core::package::read_package;
use camara_core::pipeline::{frame_to_rgb, process, Options};
use camara_core::png::write_rgb;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const PAGE: &str = include_str!("dashboard.html");
const MAX_BODY: usize = 1_500_000_000;
const KEEP: usize = 6;

#[derive(Default)]
struct Store {
    images: HashMap<String, Vec<u8>>,
    order: Vec<String>,
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
}

fn encode_png(w: usize, h: usize, px: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    write_rgb(&mut v, w, h, px).expect("png");
    v
}

fn handle(mut stream: TcpStream, store: Arc<Mutex<Store>>, counter: Arc<AtomicU64>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }

    // Separar la ruta de los parámetros (?detail=0|1|2).
    let (route, query) = match path.split_once('?') {
        Some((r, q)) => (r.to_string(), q.to_string()),
        None => (path.clone(), String::new()),
    };
    let detail: u8 = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("detail="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    match (method.as_str(), route.as_str()) {
        ("GET", "/") => respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
        ("POST", "/process") => {
            if content_length == 0 || content_length > MAX_BODY {
                respond(&mut stream, "400 Bad Request", "application/json", br#"{"error":"archivo vacio o demasiado grande"}"#);
                return;
            }
            let mut body = vec![0u8; content_length];
            if reader.read_exact(&mut body).is_err() {
                respond(&mut stream, "400 Bad Request", "application/json", br#"{"error":"subida incompleta"}"#);
                return;
            }
            let frames = match read_package(&mut &body[..]) {
                Ok(f) => f,
                Err(e) => {
                    let msg = format!(r#"{{"error":"paquete no valido: {}"}}"#, e.to_string().replace('"', "'"));
                    respond(&mut stream, "400 Bad Request", "application/json", msg.as_bytes());
                    return;
                }
            };
            drop(body);
            let n = frames.len();
            let (w0, h0) = (frames[0].width, frames[0].height);
            let t0 = Instant::now();
            let out = process(&frames, &Options::with_detail(detail));
            let secs = t0.elapsed().as_secs_f32();
            let Some(rgb) = out.rgb.as_ref() else {
                respond(&mut stream, "400 Bad Request", "application/json", br#"{"error":"el paquete no trae color (v1)"}"#);
                return;
            };
            let merged_png = encode_png(rgb.width, rgb.height, &rgb.pixels);
            let single_png = frame_to_rgb(&frames[out.reference_index])
                .map(|s| encode_png(s.width, s.height, &s.pixels))
                .unwrap_or_default();
            let id = format!("{}", counter.fetch_add(1, Ordering::SeqCst));
            {
                let mut st = store.lock().unwrap();
                st.images.insert(format!("{id}/merged.png"), merged_png);
                st.images.insert(format!("{id}/single.png"), single_png);
                st.order.push(id.clone());
                while st.order.len() > KEEP {
                    let old = st.order.remove(0);
                    st.images.remove(&format!("{old}/merged.png"));
                    st.images.remove(&format!("{old}/single.png"));
                }
            }
            let json = format!(
                r#"{{"id":"{id}","frames":{n},"width":{},"height":{},"seconds":{:.2},"reference":{},"noise_a":{:.3},"noise_b":{:.3},"src_w":{w0},"src_h":{h0},"hdr":{}}}"#,
                rgb.width, rgb.height, secs, out.reference_index, out.noise.a, out.noise.b, out.hdr
            );
            respond(&mut stream, "200 OK", "application/json", json.as_bytes());
        }
        ("GET", p) if p.starts_with("/img/") => {
            let key = &p[5..];
            let st = store.lock().unwrap();
            match st.images.get(key) {
                Some(b) if !b.is_empty() => respond(&mut stream, "200 OK", "image/png", b),
                _ => respond(&mut stream, "404 Not Found", "text/plain", b"no existe"),
            }
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", b"no existe"),
    }
}

pub fn serve(port: u16) {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("No se pudo abrir el puerto {port}: {e}");
            return;
        }
    };
    println!("Dashboard listo en  http://127.0.0.1:{port}");
    println!("(Ctrl+C para cerrar)");
    let store = Arc::new(Mutex::new(Store::default()));
    let counter = Arc::new(AtomicU64::new(1));
    for stream in listener.incoming().flatten() {
        let store = store.clone();
        let counter = counter.clone();
        std::thread::spawn(move || handle(stream, store, counter));
    }
}
