//! Системный звук: сырой PCM из scap → Opus 20 мс пакеты для паблишера.
//!
//! Нормализуем к 48 kHz stereo f32, режем на 20 мс кадры (960 сэмплов/канал),
//! кодируем Opus, считаем RFC 6464 уровень (dBov). macOS-нюанс: scap отдаёт
//! стерео ПЛАНАРНО ([L..][R..]) вопреки `is_planar()==false` — интерливим.

use std::collections::VecDeque;
use std::time::Instant;

use matehub_rtc_client::EncodedAudioFrame;
use opus::{Application, Channels, Encoder};
use scap::frame::AudioFormat;

use crate::capture::RawAudio;
use crate::dsp::{FRAME_INTERLEAVED, FRAME_SAMPLES, RATE, dbov, resample_linear};

pub struct AudioEncoder {
    enc: Encoder,
    /// Интерливленный (LRLR) f32 @48kHz stereo аккумулятор.
    acc: VecDeque<f32>,
    rtp_ts: u64,
    out_buf: Vec<u8>,
    /// Остаток от линейного ресемпла (дробная позиция чтения).
    resample_pos: f64,
}

impl AudioEncoder {
    pub fn new() -> Result<Self, String> {
        let enc = Encoder::new(RATE, Channels::Stereo, Application::Audio)
            .map_err(|e| format!("opus encoder: {e}"))?;
        Ok(Self {
            enc,
            acc: VecDeque::with_capacity(FRAME_INTERLEAVED * 4),
            rtp_ts: 0,
            out_buf: vec![0u8; 4000],
            resample_pos: 0.0,
        })
    }

    /// Скармливает сырой аудиокадр, вызывает `out` на каждый готовый 20 мс
    /// Opus-пакет. Ошибки формата/кодека логируются, кадр пропускается.
    pub fn push(&mut self, a: &RawAudio, mut out: impl FnMut(EncodedAudioFrame)) {
        let stereo48 = match to_stereo_48k(a, &mut self.resample_pos) {
            Some(s) => s,
            None => return,
        };
        self.acc.extend(stereo48);

        while self.acc.len() >= FRAME_INTERLEAVED {
            let frame: Vec<f32> = self.acc.drain(..FRAME_INTERLEAVED).collect();
            let n = match self.enc.encode_float(&frame, &mut self.out_buf) {
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("opus encode: {e}");
                    continue;
                }
            };
            let dbov = dbov(&frame);
            out(EncodedAudioFrame {
                data: self.out_buf[..n].into(),
                rtp_time_48khz: self.rtp_ts,
                captured_at: Instant::now(),
                audio_level_dbov: dbov,
                voice: dbov < 50, // громче -50 dBov ≈ речь
            });
            self.rtp_ts += FRAME_SAMPLES as u64;
        }
    }
}

/// Любой вход → интерливленный stereo f32 @48kHz. `None` при неизвестном
/// формате. `resample_pos` держит дробную позицию между вызовами.
fn to_stereo_48k(a: &RawAudio, resample_pos: &mut f64) -> Option<Vec<f32>> {
    let ch = a.channels.max(1) as usize;
    let samples = to_f32(a)?; // плоский буфер сэмплов в исходной раскладке
    if samples.is_empty() {
        return Some(Vec::new());
    }

    // → интерливленный stereo на исходной частоте.
    let planar = is_planar(a);
    let mut stereo: Vec<f32> = Vec::with_capacity(a.sample_count * 2);
    let n = a.sample_count;
    for i in 0..n {
        let (l, r) = match (ch, planar) {
            (1, _) => {
                let s = *samples.get(i)?;
                (s, s)
            }
            (_, true) => {
                // [L0..Ln-1][R0..Rn-1]
                let l = *samples.get(i).unwrap_or(&0.0);
                let r = *samples.get(n + i).unwrap_or(&0.0);
                (l, r)
            }
            (_, false) => {
                // LRLR...
                let l = *samples.get(i * ch).unwrap_or(&0.0);
                let r = *samples.get(i * ch + 1).unwrap_or(&0.0);
                (l, r)
            }
        };
        stereo.push(l);
        stereo.push(r);
    }

    if a.rate == RATE {
        *resample_pos = 0.0;
        return Some(stereo);
    }
    Some(resample_linear(&stereo, a.rate, RATE, resample_pos))
}

fn is_planar(a: &RawAudio) -> bool {
    // macOS: SCK отдаёт планарно, но planes()==1 (баг scap). Иначе доверяем.
    #[cfg(target_os = "macos")]
    {
        a.channels >= 2
    }
    #[cfg(not(target_os = "macos"))]
    {
        a.planes > 1
    }
}

/// Сырые байты → f32 сэмплы (в исходной раскладке каналов).
fn to_f32(a: &RawAudio) -> Option<Vec<f32>> {
    match a.format {
        AudioFormat::F32 => Some(bytemuck::cast_slice::<u8, f32>(&a.data).to_vec()),
        AudioFormat::I16 => Some(
            bytemuck::cast_slice::<u8, i16>(&a.data)
                .iter()
                .map(|&s| f32::from(s) / 32768.0)
                .collect(),
        ),
        AudioFormat::I32 => Some(
            bytemuck::cast_slice::<u8, i32>(&a.data)
                .iter()
                .map(|&s| s as f32 / 2_147_483_648.0)
                .collect(),
        ),
        other => {
            tracing::warn!(?other, "unsupported audio format, dropping frame");
            None
        }
    }
}
