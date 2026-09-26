#[derive(Clone)]
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
}
impl Rendered {
    pub fn rgb8(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flatten()
            .map(|v| (v.clamp(0., 1.) * 255. + 0.5) as u8)
            .collect()
    }
    pub fn rgb16(&self) -> Vec<u16> {
        self.pixels
            .iter()
            .flatten()
            .map(|v| (v.clamp(0., 1.) * 65535. + 0.5) as u16)
            .collect()
    }
    pub fn histogram(&self) -> [[u32; 256]; 3] {
        let mut h = [[0; 256]; 3];
        for p in &self.pixels {
            for c in 0..3 {
                h[c][(p[c].clamp(0., 1.) * 255.) as usize] += 1;
            }
        }
        h
    }
}
