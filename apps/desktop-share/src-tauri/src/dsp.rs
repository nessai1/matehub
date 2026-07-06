//! Мелкие DSP-примитивы, общие для system-audio (`audio.rs`) и нативного
//! войса (`voice.rs`): линейный ресемпл интерливленного stereo и RFC 6464
//! уровень громкости.

pub const RATE: u32 = 48_000;
/// 20 мс при 48 kHz stereo = 960 сэмплов/канал = 1920 интерливленных.
pub const FRAME_SAMPLES: usize = 960;
pub const FRAME_INTERLEAVED: usize = FRAME_SAMPLES * 2;

/// Линейный ресемпл интерливленного stereo `from`→`to`. `pos` держит дробную
/// позицию чтения между вызовами (непрерывность на границах буферов).
/// Дёшево, достаточно для голоса; музыкальное качество — позже `rubato`.
pub fn resample_linear(input: &[f32], from: u32, to: u32, pos: &mut f64) -> Vec<f32> {
    let frames_in = input.len() / 2;
    if frames_in == 0 {
        return Vec::new();
    }
    if from == to {
        *pos = 0.0;
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let mut out = Vec::with_capacity(((frames_in as f64 / ratio) as usize + 1) * 2);
    let mut p = *pos;
    while (p as usize) + 1 < frames_in {
        let idx = p as usize;
        let frac = (p - idx as f64) as f32;
        for c in 0..2 {
            let a = input[idx * 2 + c];
            let b = input[(idx + 1) * 2 + c];
            out.push(a + (b - a) * frac);
        }
        p += ratio;
    }
    *pos = (p - frames_in as f64).max(0.0);
    out
}

/// RFC 6464 уровень: -dBov в диапазоне 0..127 (0 = максимум, 127 = тишина).
pub fn dbov(interleaved: &[f32]) -> u8 {
    if interleaved.is_empty() {
        return 127;
    }
    let sum_sq: f64 = interleaved
        .iter()
        .map(|&s| f64::from(s) * f64::from(s))
        .sum();
    let rms = (sum_sq / interleaved.len() as f64).sqrt();
    if rms <= 1e-9 {
        return 127;
    }
    let db = 20.0 * rms.log10(); // ≤ 0
    (-db).clamp(0.0, 127.0) as u8
}

/// Складывает `src` в `dst` сэмпл-в-сэмпл (микс), клампя к [-1, 1].
/// Хардклип достаточен для v1: одновременный пик ≤3 говорящих редок и краток.
pub fn mix_into(dst: &mut [f32], src: &[f32]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d = (*d + *s).clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbov_bounds() {
        assert_eq!(dbov(&[1.0; 960]), 0);
        assert_eq!(dbov(&[0.0; 960]), 127);
        assert_eq!(dbov(&[]), 127);
        let half = dbov(&[0.5; 960]);
        assert!((5..=7).contains(&half), "got {half}");
    }

    #[test]
    fn resample_halves_at_double_rate() {
        let input: Vec<f32> = (0..8).map(|i| i as f32).collect(); // 4 stereo frames
        let mut pos = 0.0;
        let out = resample_linear(&input, 96_000, 48_000, &mut pos);
        assert!(out.len().is_multiple_of(2));
        assert!(
            (1..=2).contains(&(out.len() / 2)),
            "got {} frames",
            out.len() / 2
        );
    }

    #[test]
    fn resample_noop_same_rate() {
        let input = vec![1.0f32, 2.0, 3.0, 4.0];
        let mut pos = 0.0;
        assert_eq!(resample_linear(&input, 48_000, 48_000, &mut pos), input);
    }

    #[test]
    fn mix_sums_and_clamps() {
        let mut dst = vec![0.5f32, -0.5, 0.9];
        mix_into(&mut dst, &[0.6, -0.6, 0.2]);
        assert_eq!(dst, vec![1.0, -1.0, 1.0]);
    }
}
