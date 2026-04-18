use serde::{Deserialize, Serialize};

/// Attachment processing status.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentStatus {
    /// Ready to display (default for images/audio/docs, set after successful transcode).
    #[default]
    Ready,
    /// Video is being transcoded -- client should show loading state.
    Transcoding,
    /// Transcoding failed -- client should show error.
    Failed,
}

/// Structured media attachment.
///
/// Serialized as JSON string and stored in ScyllaDB `messages.attachments`
/// (list<text> column). Parsing is backwards-compatible: old URL-only strings
/// are recognized and returned as `{url, ...}` with empty metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// Unique ID (snowflake), also used as S3 key component.
    pub id: String,
    /// Direct URL (S3 public link or streaming endpoint).
    pub url: String,
    /// MIME type (image/jpeg, video/mp4, audio/ogg, application/pdf, ...).
    pub content_type: String,
    /// Original filename.
    pub name: String,
    /// Byte size.
    pub size: u64,
    /// Image/video width in pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Image/video height in pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Video/audio duration in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f32>,
    /// Thumbnail URL (for images/videos).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_url: Option<String>,
    /// Processing status. Defaults to "ready" for formats not needing processing.
    #[serde(default)]
    pub status: AttachmentStatus,
}

impl Attachment {
    /// Parse from the string stored in ScyllaDB.
    /// Tries JSON first, falls back to treating the string as a plain URL.
    pub fn from_stored(s: &str) -> Self {
        if let Ok(a) = serde_json::from_str::<Attachment>(s) {
            return a;
        }
        // Legacy format: bare URL
        Self {
            id: String::new(),
            url: s.to_string(),
            content_type: String::new(),
            name: String::new(),
            size: 0,
            width: None,
            height: None,
            duration: None,
            thumb_url: None,
            status: AttachmentStatus::Ready,
        }
    }

    /// MIME types that browsers don't play natively -- need transcoding to mp4/h264.
    pub fn needs_transcode(content_type: &str) -> bool {
        matches!(
            content_type,
            "video/quicktime" | "video/x-msvideo" | "video/x-matroska" | "video/3gpp" | "video/x-ms-wmv"
        )
    }

    /// Serialize to JSON string for ScyllaDB storage.
    pub fn to_stored(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Classify by content_type prefix.
    pub fn kind(&self) -> AttachmentKind {
        if self.content_type.starts_with("image/") {
            AttachmentKind::Image
        } else if self.content_type.starts_with("video/") {
            AttachmentKind::Video
        } else if self.content_type.starts_with("audio/") {
            AttachmentKind::Audio
        } else {
            AttachmentKind::Document
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    Video,
    Audio,
    Document,
}
