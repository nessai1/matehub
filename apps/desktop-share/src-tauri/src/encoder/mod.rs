//! Кодирование экрана в H.264 Annex B под screen content.
//!
//! Абстракция [`VideoEncoder`] прячет бэкенд: HW-энкодер платформы, когда
//! доступен (меньше нагрузки на CPU, гарантированный 60fps), иначе —
//! software OpenH264 (собирается везде, работает без железа).
//!
//! Выбор — [`select`]: пробует HW, при любой осечке молча падает на
//! OpenH264. Тип стёрт за `Box<dyn VideoEncoder>`, поэтому пайплайн
//! (`pipeline.rs`) бэкенд не различает.

#[cfg(all(feature = "nvenc", any(target_os = "windows", target_os = "linux")))]
mod nvenc;
mod openh264;
#[cfg(target_os = "macos")]
mod videotoolbox;
mod yuv;

use crate::capture::RawFrame;

/// Ниже опускаться бессмысленно: текст на экране превращается в кашу.
pub(crate) const MIN_BITRATE_BPS: u32 = 500_000;
/// Гистерезис ретаргета: не дёргаем энкодер на дребезг BWE < 25%.
pub(crate) const RETARGET_THRESHOLD: f64 = 0.25;

/// Один закодированный кадр — access unit H.264 в Annex B (start codes).
/// Для IDR перед слайсом идут SPS+PPS: str0m закеширует их и выдаст STAP-A.
pub struct EncodedFrame {
    pub annexb: Vec<u8>,
    pub keyframe: bool,
}

/// H.264-энкодер экрана. Реализации не потокобезопасны внутри, но владеются
/// одним capture-потоком (`Send` достаточно).
pub trait VideoEncoder: Send {
    /// BGRA-кадр → Annex B. `Ok(None)` — rate control пропустил кадр
    /// (не ошибка). Смена размеров источника переживается внутри реализации.
    fn encode(&mut self, frame: &RawFrame) -> Result<Option<EncodedFrame>, String>;

    /// Следующий кадр — IDR (ответ на PLI от SFU).
    fn force_keyframe(&mut self);

    /// Подстроить целевой битрейт под оценку BWE (bps). Реализация вправе
    /// проигнорировать мелкие изменения (гистерезис).
    fn maybe_retarget(&mut self, estimate_bps: u64) -> Result<(), String>;
}

/// Выбирает лучший доступный энкодер с graceful fallback на OpenH264.
///
/// macOS: сначала VideoToolbox (аппаратный H.264 через системный фреймворк —
/// собирается на любом Mac, работает без дискретной GPU). При ошибке
/// инициализации (например, экзотический размер) — OpenH264.
pub fn select(
    width: usize,
    height: usize,
    fps: u32,
    bitrate_bps: u32,
) -> Result<Box<dyn VideoEncoder>, String> {
    #[cfg(target_os = "macos")]
    {
        match videotoolbox::VideoToolboxEncoder::new(width, height, fps, bitrate_bps) {
            Ok(enc) => {
                tracing::info!("using VideoToolbox hardware encoder");
                return Ok(Box::new(enc));
            }
            Err(e) => {
                tracing::warn!("VideoToolbox unavailable ({e}), falling back to OpenH264");
            }
        }
    }

    #[cfg(all(feature = "nvenc", any(target_os = "windows", target_os = "linux")))]
    {
        match nvenc::NvencEncoder::new(width, height, fps, bitrate_bps) {
            Ok(enc) => {
                tracing::info!("using NVENC hardware encoder");
                return Ok(Box::new(enc));
            }
            Err(e) => {
                tracing::warn!("NVENC unavailable ({e}), falling back to OpenH264");
            }
        }
    }

    let enc = openh264::OpenH264Encoder::new(width, height, fps, bitrate_bps)?;
    tracing::info!("using OpenH264 software encoder");
    Ok(Box::new(enc))
}
