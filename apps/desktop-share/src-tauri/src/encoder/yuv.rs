//! BGRA → I420 (BT.601 limited range) и AVCC → Annex B.
//!
//! Общее между software- и HW-путями: OpenH264 конвертит внутри своей
//! `YUVBuffer`, а VideoToolbox/NVENC хотят голые планы — их кормит [`Bgra2I420`].

/// Переиспользуемый конвертер BGRA→I420: планы живут между кадрами, чтобы
/// не аллоцировать на каждом кадре (60 fps).
pub struct Bgra2I420 {
    w: usize,
    h: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Bgra2I420 {
    pub fn new(w: usize, h: usize) -> Self {
        let (w, h) = (w & !1, h & !1);
        Self {
            w,
            h,
            y: vec![0; w * h],
            u: vec![0; (w / 2) * (h / 2)],
            v: vec![0; (w / 2) * (h / 2)],
        }
    }

    pub fn dims(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// Конвертит плотный BGRA (stride = `src_width*4`) в свои I420-планы.
    /// Правый/нижний нечётный край источника отбрасывается (≤1px).
    pub fn convert(&mut self, bgra: &[u8], src_width: usize) {
        let (w, h) = (self.w, self.h);
        let cw = w / 2;
        for row in 0..h {
            for col in 0..w {
                let i = (row * src_width + col) * 4;
                let b = i32::from(bgra[i]);
                let g = i32::from(bgra[i + 1]);
                let r = i32::from(bgra[i + 2]);
                let y = (66 * r + 129 * g + 25 * b + 128 + (16 << 8)) >> 8;
                self.y[row * w + col] = y.clamp(16, 235) as u8;
                if row % 2 == 0 && col % 2 == 0 {
                    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                    let ci = (row / 2) * cw + col / 2;
                    self.u[ci] = u.clamp(16, 240) as u8;
                    self.v[ci] = v.clamp(16, 240) as u8;
                }
            }
        }
    }
}

/// AVCC (NAL с 4-байтовым BE-префиксом длины) → Annex B (start codes).
/// Для keyframe SPS/PPS выкладываются впереди — декодер зрителя может войти
/// в поток на любом IDR (str0m их кеширует и отдаёт STAP-A).
pub fn avcc_to_annexb(data: &[u8], sps: &[Vec<u8>], pps: &[Vec<u8>], keyframe: bool) -> Vec<u8> {
    const START: [u8; 4] = [0, 0, 0, 1];
    let mut out = Vec::with_capacity(data.len() + 64);
    if keyframe {
        for nal in sps.iter().chain(pps.iter()) {
            out.extend_from_slice(&START);
            out.extend_from_slice(nal);
        }
    }
    let mut i = 0;
    while i + 4 <= data.len() {
        let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        i += 4;
        if len == 0 || i + len > data.len() {
            break;
        }
        out.extend_from_slice(&START);
        out.extend_from_slice(&data[i..i + len]);
        i += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_prepends_param_sets_on_keyframe() {
        // Один NAL длиной 3 (0xAA 0xBB 0xCC) в AVCC.
        let data = [0, 0, 0, 3, 0xAA, 0xBB, 0xCC];
        let sps = vec![vec![0x67, 0x42]];
        let pps = vec![vec![0x68, 0xCE]];
        let out = avcc_to_annexb(&data, &sps, &pps, true);
        assert_eq!(
            out,
            vec![
                0, 0, 0, 1, 0x67, 0x42, // SPS
                0, 0, 0, 1, 0x68, 0xCE, // PPS
                0, 0, 0, 1, 0xAA, 0xBB, 0xCC, // slice
            ]
        );
    }

    #[test]
    fn avcc_skips_param_sets_on_delta() {
        let data = [0, 0, 0, 2, 0x41, 0x9A];
        let out = avcc_to_annexb(&data, &[vec![0x67]], &[vec![0x68]], false);
        assert_eq!(out, vec![0, 0, 0, 1, 0x41, 0x9A]);
    }

    #[test]
    fn avcc_truncated_length_is_ignored() {
        // Длина 10, но байтов только 2 — обрубок не паникует, а отбрасывается.
        let data = [0, 0, 0, 10, 0x41, 0x9A];
        let out = avcc_to_annexb(&data, &[], &[], false);
        assert!(out.is_empty());
    }
}
