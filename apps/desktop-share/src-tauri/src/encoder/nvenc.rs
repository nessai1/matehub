//! NVENC — аппаратный H.264 (Windows/Linux + NVIDIA). Feature-gated `nvenc`.
//!
//! `shiguredo_nvcodec` грузит CUDA-драйвер через dlopen: крейт собирается на
//! GPU-less раннере, а при отсутствии драйвера падает в `Encoder::new` →
//! `encoder::select` уходит на OpenH264. NVENC H.264 отдаёт Annex B нативно;
//! на IDR просим `output_spspps`, чтобы SPS/PPS были в потоке.
//!
//! Крейт callback-based (worker-поток зовёт handler), поэтому AU складываем в
//! канал и сливаем в `pending` под pull-контракт трейта.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};

use shiguredo_nvcodec::{
    BufferFormat, CodecConfig, EncodeOptions, Encoder, EncoderConfig, FnEncodeHandler,
    H264EncoderConfig, H264Profile, PictureType, Preset, RateControlMode, ReconfigureParams,
    TuningInfo,
};

use super::yuv::Bgra2I420;
use super::{EncodedFrame, MIN_BITRATE_BPS, RETARGET_THRESHOLD, VideoEncoder};
use crate::capture::RawFrame;

type NvResult = Result<shiguredo_nvcodec::EncodedFrame<()>, shiguredo_nvcodec::Error>;

pub struct NvencEncoder {
    encoder: Encoder<FnEncodeHandler<()>>,
    rx: Receiver<EncodedFrame>,
    conv: Bgra2I420,
    current_bitrate: u32,
    max_bitrate: u32,
    force_idr: bool,
    pending: VecDeque<EncodedFrame>,
}

fn build_config(w: usize, h: usize, fps: u32, bitrate: u32) -> EncoderConfig {
    EncoderConfig {
        codec: CodecConfig::H264(H264EncoderConfig {
            profile: Some(H264Profile::High),
            idr_period: None, // = gop_length; длинный GOP + IDR по запросу
        }),
        width: w as u32,
        height: h as u32,
        max_encode_width: None,
        max_encode_height: None,
        framerate_num: fps,
        framerate_den: 1,
        average_bitrate: Some(bitrate),
        preset: Preset::P4, // баланс скорость/качество для realtime
        tuning_info: TuningInfo::LOW_LATENCY,
        rate_control_mode: RateControlMode::Cbr,
        gop_length: Some((fps * 10).max(1)),
        frame_interval_p: 1,               // без B-кадров → низкая задержка
        buffer_format: BufferFormat::Iyuv, // I420 — тот же конвертер, что у VT
        device_id: 0,
    }
}

fn make_encoder(
    cfg: EncoderConfig,
) -> Result<(Encoder<FnEncodeHandler<()>>, Receiver<EncodedFrame>), String> {
    let (tx, rx): (Sender<EncodedFrame>, Receiver<EncodedFrame>) = channel();
    let handler = FnEncodeHandler::new(move |result: NvResult| {
        if let Ok(frame) = result {
            let keyframe = matches!(frame.picture_type(), PictureType::Idr | PictureType::I);
            // NVENC H.264 — уже Annex B, копируем как есть.
            let _ = tx.send(EncodedFrame {
                annexb: frame.data().to_vec(),
                keyframe,
            });
        } else if let Err(e) = result {
            tracing::warn!("NVENC callback error: {e}");
        }
    });
    let encoder = Encoder::new(cfg, handler).map_err(|e| format!("NVENC init: {e}"))?;
    Ok((encoder, rx))
}

impl NvencEncoder {
    pub fn new(width: usize, height: usize, fps: u32, bitrate_bps: u32) -> Result<Self, String> {
        let conv = Bgra2I420::new(width, height);
        let (w, h) = conv.dims();
        if w == 0 || h == 0 {
            return Err("zero-sized frame".into());
        }
        let (encoder, rx) = make_encoder(build_config(w, h, fps, bitrate_bps))?;
        Ok(Self {
            encoder,
            rx,
            conv,
            current_bitrate: bitrate_bps,
            max_bitrate: bitrate_bps,
            force_idr: false,
            pending: VecDeque::new(),
        })
    }

    fn drain(&mut self) {
        while let Ok(f) = self.rx.try_recv() {
            self.pending.push_back(f);
        }
    }
}

impl VideoEncoder for NvencEncoder {
    fn encode(&mut self, frame: &RawFrame) -> Result<Option<EncodedFrame>, String> {
        if frame.bgra.len() < frame.width * frame.height * 4 {
            return Ok(None);
        }
        self.conv.convert(&frame.bgra, frame.width);
        let idr = std::mem::take(&mut self.force_idr);
        let options = EncodeOptions {
            force_intra: idr,
            force_idr: idr,
            output_spspps: idr, // на IDR обязательно вынести SPS/PPS в поток
        };
        // I420 планы подряд: Y, U, V (BufferFormat::Iyuv).
        let mut buf = Vec::with_capacity(self.conv.y.len() + self.conv.u.len() + self.conv.v.len());
        buf.extend_from_slice(&self.conv.y);
        buf.extend_from_slice(&self.conv.u);
        buf.extend_from_slice(&self.conv.v);
        self.encoder
            .encode(&buf, &options, ())
            .map_err(|e| format!("NVENC encode: {e}"))?;
        self.drain();
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
        self.encoder
            .reconfigure(ReconfigureParams {
                width: None,
                height: None,
                framerate_num: None,
                framerate_den: None,
                average_bitrate: Some(target),
                max_bitrate: Some(target),
            })
            .map_err(|e| format!("NVENC reconfigure: {e}"))?;
        self.current_bitrate = target;
        tracing::info!(to = target, "NVENC bitrate retargeted");
        Ok(())
    }
}
