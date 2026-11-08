//! Formato de paquete de ráfaga (.camb). Lo escribe el celular y lo lee el motor.
//!
//! Cabecera: "CAMB" | u32 versión | u32 nº de fotos
//!
//! v1 por foto: u32 ancho | u32 alto | u32 blanco | u32 ISO | u64 exposición(ns)
//!              | ancho*alto muestras u16 LE (luminancia)
//!
//! v2 por foto: u32 ancho | u32 alto | u32 blanco | u32 ISO | u64 exposición(ns)
//!              | u32 rotación horaria (grados) | u32 cw | u32 ch
//!              | Y: ancho*alto u8 | Cb: cw*ch u8 | Cr: cw*ch u8   (YUV 4:2:0, 8 bits)

use std::io::{self, Read, Write};

pub const MAGIC: &[u8; 4] = b"CAMB";
pub const VERSION: u32 = 2;

#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub white_level: u32,
    pub iso: u32,
    pub exposure_ns: u64,
    /// Grados en sentido horario para dejar la foto derecha.
    pub rotation: u32,
    pub luma: Vec<u16>,
    /// Dimensiones del croma (0 si no hay color).
    pub cw: u32,
    pub ch: u32,
    pub cb: Vec<u16>,
    pub cr: Vec<u16>,
}

fn rd_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn rd_u64<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn rd_u8s<R: Read>(r: &mut R, n: usize) -> io::Result<Vec<u16>> {
    let mut raw = vec![0u8; n];
    r.read_exact(&mut raw)?;
    Ok(raw.into_iter().map(|v| v as u16).collect())
}

fn to_u8s(v: &[u16]) -> Vec<u8> {
    v.iter().map(|&x| x.min(255) as u8).collect()
}

pub fn write_package<W: Write>(w: &mut W, frames: &[Frame]) -> io::Result<()> {
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&(frames.len() as u32).to_le_bytes())?;
    for f in frames {
        w.write_all(&f.width.to_le_bytes())?;
        w.write_all(&f.height.to_le_bytes())?;
        w.write_all(&f.white_level.to_le_bytes())?;
        w.write_all(&f.iso.to_le_bytes())?;
        w.write_all(&f.exposure_ns.to_le_bytes())?;
        w.write_all(&f.rotation.to_le_bytes())?;
        w.write_all(&f.cw.to_le_bytes())?;
        w.write_all(&f.ch.to_le_bytes())?;
        w.write_all(&to_u8s(&f.luma))?;
        if f.cw > 0 {
            w.write_all(&to_u8s(&f.cb))?;
            w.write_all(&to_u8s(&f.cr))?;
        }
    }
    Ok(())
}

pub fn read_package<R: Read>(r: &mut R) -> io::Result<Vec<Frame>> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "no es un paquete CAMB"));
    }
    let ver = rd_u32(r)?;
    if ver != 1 && ver != 2 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "versión de paquete no soportada"));
    }
    let n = rd_u32(r)? as usize;
    if n == 0 || n > 64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "número de fotos inválido"));
    }
    let mut frames = Vec::with_capacity(n);
    for _ in 0..n {
        let width = rd_u32(r)?;
        let height = rd_u32(r)?;
        let white_level = rd_u32(r)?;
        let iso = rd_u32(r)?;
        let exposure_ns = rd_u64(r)?;
        if width == 0 || height == 0 || width > 20000 || height > 20000 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "dimensiones inválidas"));
        }
        let count = width as usize * height as usize;
        if ver == 1 {
            let mut raw = vec![0u8; count * 2];
            r.read_exact(&mut raw)?;
            let luma = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            frames.push(Frame { width, height, white_level, iso, exposure_ns, luma, ..Default::default() });
        } else {
            let rotation = rd_u32(r)?;
            let cw = rd_u32(r)?;
            let ch = rd_u32(r)?;
            let luma = rd_u8s(r, count)?;
            let (cb, cr) = if cw > 0 && ch > 0 {
                let cn = cw as usize * ch as usize;
                (rd_u8s(r, cn)?, rd_u8s(r, cn)?)
            } else {
                (Vec::new(), Vec::new())
            };
            frames.push(Frame { width, height, white_level, iso, exposure_ns, rotation, luma, cw, ch, cb, cr });
        }
    }
    Ok(frames)
}
