//! Plano de imagen en f32 con muestreo bilineal y bicúbico.

#[derive(Clone, Debug)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

#[inline]
fn catmull_rom(t: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
}

impl Plane {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, data: vec![0.0; w * h] }
    }

    pub fn from_u16(w: usize, h: usize, src: &[u16]) -> Self {
        assert_eq!(src.len(), w * h);
        Self { w, h, data: src.iter().map(|&v| v as f32).collect() }
    }

    /// Lectura con bordes replicados.
    #[inline]
    pub fn get(&self, x: isize, y: isize) -> f32 {
        let xc = x.clamp(0, self.w as isize - 1) as usize;
        let yc = y.clamp(0, self.h as isize - 1) as usize;
        self.data[yc * self.w + xc]
    }

    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let xi = x0 as isize;
        let yi = y0 as isize;
        let a = self.get(xi, yi);
        let b = self.get(xi + 1, yi);
        let c = self.get(xi, yi + 1);
        let d = self.get(xi + 1, yi + 1);
        (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy
    }

    /// Interpolación Catmull-Rom (más nítida que la bilineal).
    #[inline]
    pub fn sample_cubic(&self, x: f32, y: f32) -> f32 {
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let xi = x0 as isize;
        let yi = y0 as isize;
        let mut rows = [0.0f32; 4];
        for (j, r) in rows.iter_mut().enumerate() {
            let yy = yi + j as isize - 1;
            *r = catmull_rom(
                fx,
                self.get(xi - 1, yy),
                self.get(xi, yy),
                self.get(xi + 1, yy),
                self.get(xi + 2, yy),
            );
        }
        catmull_rom(fy, rows[0], rows[1], rows[2], rows[3])
    }

    /// Convierte a 8 bits escalando por `white`.
    pub fn to_u8(&self, white: f32) -> Vec<u8> {
        let k = 255.0 / white;
        self.data.iter().map(|&v| (v * k).round().clamp(0.0, 255.0) as u8).collect()
    }
}
