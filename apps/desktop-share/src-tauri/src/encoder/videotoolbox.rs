//! VideoToolbox — аппаратный H.264 (macOS). Системный фреймворк: собирается
//! на любом Mac и в CI без дискретной GPU.
//!
//! Крейт `shiguredo_video_toolbox` pull-based (encode → next_frame) и отдаёт
//! AVCC + отдельные SPS/PPS; мы конвертим в Annex B ([`super::yuv`]). Вход —
//! I420, поэтому BGRA конвертируем сами (VT тоже умеет BGRA-CVPixelBuffer, но
//! этот крейт принимает только планы).

use std::collections::VecDeque;
use std::num::NonZeroU32;

use shiguredo_video_toolbox::{
    CodecConfig, EncodeOptions, Encoder, EncoderConfig, FrameData, H264EncoderConfig,
    H264EntropyMode, H264Profile, PixelFormat,
};

use super::yuv::{Bgra2I420, avcc_to_annexb};
use super::{EncodedFrame, MIN_BITRATE_BPS, RETARGET_THRESHOLD, VideoEncoder};
use crate::capture::RawFrame;

pub struct VideoToolboxEncoder {
    encoder: Encoder,
    conv: Bgra2I420,
    fps: u32,
    current_bitrate: u32,
    max_bitrate: u32,
    force_idr: bool,
    /// VT может отдавать AU не строго 1:1 с submit — буферизуем, чтобы не
    /// терять кадры (pipeline забирает по одному на encode()).
    pending: VecDeque<EncodedFrame>,
}

fn build_config(w: usize, h: usize, fps: u32, bitrate: u32) -> EncoderConfig {
    EncoderConfig {
        width: w as u32,
        height: h as u32,
        codec: CodecConfig::H264(H264EncoderConfig {
            // High + CABAC: декодируется всеми зрителями (вкл. Safari),
            // лучшее качество текста на том же битрейте.
            profile: H264Profile::High,
            entropy_mode: H264EntropyMode::Cabac,
        }),
        pixel_format: PixelFormat::I420,
        average_bitrate: Some(u64::from(bitrate)),
        fps_numerator: fps,
        fps_denominator: 1,
        // Screen content, низкая задержка: скорость > качество, без B-кадров.
        prioritize_encoding_speed_over_quality: true,
        real_time: true,
        maximize_power_efficiency: false,
        allow_frame_reordering: false,
        allow_temporal_compression: true,
        // Длинный GOP: IDR по запросу (PLI от SFU), а не по таймеру (§6.3).
        max_key_frame_interval: NonZeroU32::new((fps * 10).max(1)),
        max_key_frame_interval_duration: None,
        max_frame_delay_count: None,
    }
}

impl VideoToolboxEncoder {
    pub fn new(width: usize, height: usize, fps: u32, bitrate_bps: u32) -> Result<Self, String> {
        let conv = Bgra2I420::new(width, height);
        let (w, h) = conv.dims();
        if w == 0 || h == 0 {
            return Err("zero-sized frame".into());
        }
        let encoder = Encoder::new(build_config(w, h, fps, bitrate_bps))
            .map_err(|e| format!("VTCompressionSession: {e}"))?;
        Ok(Self {
            encoder,
            conv,
            fps,
            current_bitrate: bitrate_bps,
            max_bitrate: bitrate_bps,
            force_idr: false,
            pending: VecDeque::new(),
        })
    }

    /// Сливает готовые AU из VT в `pending` (Annex B).
    fn drain(&mut self) -> Result<(), String> {
        while let Some(f) = self
            .encoder
            .next_frame()
            .map_err(|e| format!("next_frame: {e}"))?
        {
            let annexb = avcc_to_annexb(&f.data, &f.sps_list, &f.pps_list, f.keyframe);
            if !annexb.is_empty() {
                self.pending.push_back(EncodedFrame {
                    annexb,
                    keyframe: f.keyframe,
                });
            }
        }
        Ok(())
    }
}

impl VideoEncoder for VideoToolboxEncoder {
    fn encode(&mut self, frame: &RawFrame) -> Result<Option<EncodedFrame>, String> {
        let (w, h) = self.conv.dims();
        if w == 0 || h == 0 {
            return Ok(None);
        }
        // Смена размеров источника → пересоздаём и сессию, и конвертер.
        let (fw, fh) = (frame.width & !1, frame.height & !1);
        if (fw, fh) != (w, h) {
            self.conv = Bgra2I420::new(frame.width, frame.height);
            let (nw, nh) = self.conv.dims();
            self.encoder = Encoder::new(build_config(nw, nh, self.fps, self.current_bitrate))
                .map_err(|e| format!("VT resize: {e}"))?;
        }
        if frame.bgra.len() < frame.width * frame.height * 4 {
            return Ok(None);
        }
        self.conv.convert(&frame.bgra, frame.width);

        let options = EncodeOptions {
            force_key_frame: std::mem::take(&mut self.force_idr),
        };
        self.encoder
            .encode(
                &FrameData::I420 {
                    y: &self.conv.y,
                    u: &self.conv.u,
                    v: &self.conv.v,
                },
                &options,
            )
            .map_err(|e| format!("VT encode: {e}"))?;

        self.drain()?;
        Ok(self.pending.pop_front())
    }

    fn force_keyframe(&mut self) {
        self.force_idr = true;
    }

    fn maybe_retarget(&mut self, estimate_bps: u64) -> Result<(), String> {
        let target = (estimate_bps as f64 * 0.85) as u32;
        let target = target.clamp(MIN_BITRATE_BPS, self.max_bitrate);
        let delta = (f64::from(target) - f64::from(self.current_bitrate)).abs()
            / f64::from(self.current_bitrate);
        if delta < RETARGET_THRESHOLD {
            return Ok(());
        }
        let (w, h) = self.conv.dims();
        // reconfigure флашит незабранные кадры — заберём их следующим drain().
        self.encoder
            .reconfigure(build_config(w, h, self.fps, target))
            .map_err(|e| format!("VT reconfigure: {e}"))?;
        self.current_bitrate = target;
        tracing::info!(to = target, "VideoToolbox bitrate retargeted");
        Ok(())
    }
}
