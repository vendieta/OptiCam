//! Escritor PNG propio (gris o RGB de 8 bits, deflate sin compresión).

use std::io::{self, Write};

fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        table[i as usize] = c;
    }
    table
}

fn crc32(table: &[u32; 256], parts: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for part in parts {
        for &b in *part {
            crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
    }
    crc ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn chunk<W: Write>(w: &mut W, table: &[u32; 256], kind: &[u8; 4], data: &[u8]) -> io::Result<()> {
    w.write_all(&(data.len() as u32).to_be_bytes())?;
    w.write_all(kind)?;
    w.write_all(data)?;
    w.write_all(&crc32(table, &[kind, data]).to_be_bytes())
}

fn write_png<W: Write>(w: &mut W, width: usize, height: usize, channels: usize, pixels: &[u8]) -> io::Result<()> {
    assert_eq!(pixels.len(), width * height * channels);
    let table = crc_table();
    w.write_all(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])?;
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    let color_type = if channels == 3 { 2 } else { 0 };
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
    chunk(w, &table, b"IHDR", &ihdr)?;

    let stride = width * channels;
    let mut raw = Vec::with_capacity((stride + 1) * height);
    for row in pixels.chunks(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 16);
    z.extend_from_slice(&[0x78, 0x01]);
    let mut blocks = raw.chunks(65535).peekable();
    while let Some(b) = blocks.next() {
        let last = blocks.peek().is_none();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    chunk(w, &table, b"IDAT", &z)?;
    chunk(w, &table, b"IEND", &[])
}

/// Imagen de grises de 8 bits.
pub fn write_gray<W: Write>(w: &mut W, width: usize, height: usize, pixels: &[u8]) -> io::Result<()> {
    write_png(w, width, height, 1, pixels)
}

/// Imagen RGB de 8 bits.
pub fn write_rgb<W: Write>(w: &mut W, width: usize, height: usize, pixels: &[u8]) -> io::Result<()> {
    write_png(w, width, height, 3, pixels)
}
