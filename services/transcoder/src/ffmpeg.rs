//! ffmpeg CLI wrapper.
//!
//! Requires `ffmpeg` and `ffprobe` binaries in PATH (system dependency).
//! In Docker: `apt-get install ffmpeg`. On macOS: `brew install ffmpeg`.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use tokio::process::Command;

pub struct VideoInfo {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f32>,
}

/// Verify ffmpeg is installed and executable.
pub async fn check_ffmpeg() -> Result<()> {
    let out = Command::new("ffmpeg")
        .arg("-version")
        .output()
        .await
        .context("ffmpeg not found in PATH -- install with `brew install ffmpeg` or apt-get install ffmpeg")?;

    if !out.status.success() {
        return Err(anyhow!("ffmpeg returned non-zero"));
    }

    let version_line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    tracing::info!(%version_line, "ffmpeg ready");
    Ok(())
}

/// Transcode any input to mp4 (h264 + aac). Returns video metadata.
pub async fn transcode_to_mp4(input: &Path, output: &Path) -> Result<VideoInfo> {
    // Fast preset, reasonable quality. No hardware accel yet (works everywhere).
    let status = Command::new("ffmpeg")
        .args([
            "-y", // overwrite output
            "-i",
            &input.to_string_lossy(),
            "-c:v",
            "libx264", // H.264 video
            "-preset",
            "veryfast", // speed over compression
            "-crf",
            "23", // quality (lower = better, 18-28 reasonable)
            "-movflags",
            "+faststart", // web streaming (moov atom at front)
            "-c:a",
            "aac", // AAC audio
            "-b:a",
            "128k", // audio bitrate
            "-pix_fmt",
            "yuv420p", // broad compatibility
            &output.to_string_lossy(),
        ])
        .status()
        .await
        .context("run ffmpeg")?;

    if !status.success() {
        return Err(anyhow!("ffmpeg exited with status {:?}", status.code()));
    }

    // Extract metadata from output
    let info = probe_video(output).await.unwrap_or(VideoInfo {
        width: None,
        height: None,
        duration: None,
    });

    Ok(info)
}

/// Extract width, height, duration via ffprobe.
async fn probe_video(path: &Path) -> Result<VideoInfo> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height:format=duration",
            "-of",
            "json",
            &path.to_string_lossy(),
        ])
        .output()
        .await
        .context("run ffprobe")?;

    if !out.status.success() {
        return Err(anyhow!("ffprobe failed"));
    }

    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let stream = json["streams"].get(0);
    let width = stream.and_then(|s| s["width"].as_u64()).map(|v| v as u32);
    let height = stream.and_then(|s| s["height"].as_u64()).map(|v| v as u32);
    let duration = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f32>().ok());

    Ok(VideoInfo {
        width,
        height,
        duration,
    })
}
