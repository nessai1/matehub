//! Обёртка над scap: перечисление источников и захват BGRA-кадров.

use serde::Serialize;

/// Источник для пикера в UI. `index` — позиция в `scap::get_all_targets()`
/// на момент листинга; перед стартом список перечитывается и матчится по
/// `(kind, id)`, чтобы пережить изменения между листингом и стартом.
#[derive(Debug, Clone, Serialize)]
pub struct TargetInfo {
    pub id: u64,
    pub kind: String, // "display" | "window"
    pub title: String,
}

pub fn is_supported() -> bool {
    scap::is_supported()
}

pub fn has_permission() -> bool {
    scap::has_permission()
}

pub fn request_permission() -> bool {
    scap::request_permission()
}

pub fn list_targets() -> Vec<TargetInfo> {
    // Без прав на запись экрана перечислять источники нечего (а на старых
    // сборках scap это ещё и панковало). Пустой список → UI покажет запрос
    // прав вместо молчаливой пустоты.
    if !scap::has_permission() {
        return Vec::new();
    }
    scap::get_all_targets()
        .into_iter()
        .map(|t| match t {
            scap::Target::Display(d) => TargetInfo {
                id: d.id as u64,
                kind: "display".into(),
                title: d.title,
            },
            scap::Target::Window(w) => TargetInfo {
                id: w.id as u64,
                kind: "window".into(),
                title: w.title,
            },
        })
        .collect()
}

pub fn find_target(kind: &str, id: u64) -> Option<scap::Target> {
    scap::get_all_targets().into_iter().find(|t| match t {
        scap::Target::Display(d) => kind == "display" && d.id as u64 == id,
        scap::Target::Window(w) => kind == "window" && w.id as u64 == id,
    })
}

/// Сырой кадр захвата (BGRA, плотная упаковка).
pub struct RawFrame {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

/// Сырой аудиокадр захвата системного звука (PCM). Планарность/формат —
/// как отдал scap; интерпретация в `audio.rs`.
pub struct RawAudio {
    pub data: Vec<u8>,
    pub sample_count: usize, // на канал
    pub channels: u16,
    pub rate: u32,
    pub format: scap::frame::AudioFormat,
    /// planes>1 = planar. На macOS scap врёт (planes==1 при planar-раскладке),
    /// поэтому там `audio::is_planar` смотрит на channels, а не сюда → dead.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub planes: u16,
}

/// Демультиплексированный кадр из единого потока scap.
pub enum Captured {
    Video(RawFrame),
    Audio(RawAudio),
}

pub struct Capture {
    capturer: scap::capturer::Capturer,
}

impl Capture {
    /// `target = None` — основной дисплей. `system_audio` включает захват
    /// системного звука (macOS 13+/Windows; Linux — как отдаст PipeWire).
    /// На Linux системный portal-пикер может переопределить выбор источника.
    pub fn start(
        target: Option<scap::Target>,
        fps: u32,
        system_audio: bool,
    ) -> Result<Self, String> {
        let options = scap::capturer::Options {
            fps,
            show_cursor: true,
            show_highlight: false,
            target,
            crop_area: None,
            output_type: scap::frame::FrameType::BGRAFrame,
            output_resolution: scap::capturer::Resolution::Captured,
            excluded_targets: None,
            captures_audio: system_audio,
            // Не захватываем звук собственного процесса (эхо себя).
            exclude_current_process_audio: true,
        };
        let mut capturer =
            scap::capturer::Capturer::build(options).map_err(|e| format!("capturer: {e}"))?;
        capturer.start_capture();
        Ok(Self { capturer })
    }

    /// Блокирующее получение следующего кадра (видео или аудио — единый
    /// scap-поток, демультиплексируем по варианту `Frame`).
    pub fn next_frame(&mut self) -> Result<Option<Captured>, String> {
        loop {
            let frame = self
                .capturer
                .get_next_frame()
                .map_err(|e| format!("get_next_frame: {e}"))?;
            match frame {
                scap::frame::Frame::Video(scap::frame::VideoFrame::BGRA(f)) => {
                    if f.width <= 0 || f.height <= 0 {
                        return Ok(None);
                    }
                    return Ok(Some(Captured::Video(RawFrame {
                        width: f.width as usize,
                        height: f.height as usize,
                        bgra: f.data,
                    })));
                }
                scap::frame::Frame::Audio(a) => {
                    return Ok(Some(Captured::Audio(RawAudio {
                        data: a.raw_data().to_vec(),
                        sample_count: a.sample_count(),
                        channels: a.channels(),
                        rate: a.rate(),
                        format: a.format(),
                        planes: a.planes(),
                    })));
                }
                // Прочие видеоформаты не запрашивали.
                _ => continue,
            }
        }
    }

    pub fn stop(&mut self) {
        self.capturer.stop_capture();
    }
}
