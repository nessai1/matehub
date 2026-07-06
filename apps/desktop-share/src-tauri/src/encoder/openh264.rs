//! Software H.264 (OpenH264) с настройками под screen content.
//!
//! Универсальный fallback: собирается и работает на всех платформах без
//! железа. HW-энкодеры (VideoToolbox / NVENC) живут рядом в модуле и
//! выбираются первыми, когда доступны (см. `encoder::select`).

use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, RateControlMode, UsageType,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};

use super::{EncodedFrame, MIN_BITRATE_BPS, RETARGET_THRESHOLD, VideoEncoder};
use crate::capture::RawFrame;

pub struct OpenH264Encoder {
    encoder: Encoder,
    width: usize,
    height: usize,
    fps: u32,
    current_bitrate: u32,
    max_bitrate: u32,
    yuv: YUVBuffer,
    /// Scratch для среза нечётного правого/нижнего края (окна бывают
    /// произвольных размеров, а 4:2:0 требует чётных).
    crop_buf: Vec<u8>,
}

fn build_encoder(bitrate: u32, fps: u32) -> Result<Encoder, String> {
    let config = EncoderConfig::new()
        .usage_type(UsageType::ScreenContentRealTime)
        .bitrate(BitRate::from_bps(bitrate))
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        .rate_control_mode(RateControlMode::Bitrate)
        // Длинный GOP: IDR по запросу (PLI от SFU), а не по таймеру —
        // ровно как рекомендует phase-2.5 §6.3.
        .intra_frame_period(IntraFramePeriod::from_num_frames(fps * 10))
        .skip_frames(true);
    Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|e| e.to_string())
}

impl OpenH264Encoder {
    pub fn new(width: usize, height: usize, fps: u32, bitrate_bps: u32) -> Result<Self, String> {
        // Энкодеру нужны чётные размеры (4:2:0 сабсемплинг).
        let width = width & !1;
        let height = height & !1;
        Ok(Self {
            encoder: build_encoder(bitrate_bps, fps)?,
            width,
            height,
            fps,
            current_bitrate: bitrate_bps,
            max_bitrate: bitrate_bps,
            yuv: YUVBuffer::new(width, height),
            crop_buf: Vec::new(),
        })
    }
}

impl VideoEncoder for OpenH264Encoder {
    /// Ретаргет битрейта по BWE. OpenH264-обёртка не отдаёт SetOption —
    /// пересоздаём энкодер (даёт свежий IDR + SPS/PPS, что после падения
    /// полосы и так полезно). Гистерезис защищает от дребезга.
    fn maybe_retarget(&mut self, estimate_bps: u64) -> Result<(), String> {
        let target = (estimate_bps as f64 * 0.85) as u32;
        let target = target.clamp(MIN_BITRATE_BPS, self.max_bitrate);
        let delta = (f64::from(target) - f64::from(self.current_bitrate)).abs()
            / f64::from(self.current_bitrate);
        if delta < RETARGET_THRESHOLD {
            return Ok(());
        }
        tracing::info!(
            from = self.current_bitrate,
            to = target,
            "re-targeting encoder bitrate (BWE)"
        );
        self.encoder = build_encoder(target, self.fps)?;
        self.current_bitrate = target;
        Ok(())
    }

    fn force_keyframe(&mut self) {
        self.encoder.force_intra_frame();
    }

    /// BGRA → I420 → H.264 Annex B. Смена размеров источника (ресайз окна,
    /// смена разрешения дисплея) переживается пересозданием YUV-буфера;
    /// сам OpenH264 переинициализируется прозрачно.
    fn encode(&mut self, frame: &RawFrame) -> Result<Option<EncodedFrame>, String> {
        let w = frame.width & !1;
        let h = frame.height & !1;
        if w == 0 || h == 0 {
            return Ok(None);
        }
        if frame.bgra.len() < frame.width * frame.height * 4 {
            tracing::warn!(
                len = frame.bgra.len(),
                w = frame.width,
                h = frame.height,
                "short BGRA frame, skipping"
            );
            return Ok(None);
        }
        if w != self.width || h != self.height {
            self.width = w;
            self.height = h;
            self.yuv = YUVBuffer::new(w, h);
        }
        if w == frame.width && h == frame.height {
            self.yuv.read_rgb(BgraSliceU8::new(&frame.bgra, (w, h)));
        } else {
            self.crop_buf.clear();
            for row in 0..h {
                let start = row * frame.width * 4;
                self.crop_buf
                    .extend_from_slice(&frame.bgra[start..start + w * 4]);
            }
            self.yuv.read_rgb(BgraSliceU8::new(&self.crop_buf, (w, h)));
        }

        let bitstream = self.encoder.encode(&self.yuv).map_err(|e| e.to_string())?;
        let annexb = bitstream.to_vec();
        if annexb.is_empty() {
            // Rate control пропустил кадр (skip_frames) — это не ошибка.
            return Ok(None);
        }
        let keyframe = matches!(
            bitstream.frame_type(),
            openh264::encoder::FrameType::IDR | openh264::encoder::FrameType::I
        );
        Ok(Some(EncodedFrame { annexb, keyframe }))
    }
}
