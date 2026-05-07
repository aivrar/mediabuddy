use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single piece of media in the library — photo, video, illustration, etc.
/// Internally still called Image for legacy reasons; the wider data model
/// supports any media kind via the `kind` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub id: String,
    pub source: String,
    pub source_id: String,
    pub kind: String, // "photo" | "video" | "illustration" | "vector"
    pub source_page_url: String,

    pub filename: String,
    pub path: String,
    pub thumb_path: String,
    pub url: String,

    /// Map of named URL tiers (e.g. "thumb", "preview", "small", "regular",
    /// "large", "full", "raw"). Keys differ per provider; we keep all the
    /// useful ones.
    pub urls: Value,

    pub width: i64,
    pub height: i64,
    pub duration_secs: Option<i64>,
    pub file_size: Option<i64>,

    pub query: String,
    pub alt: String,
    pub tags: Vec<String>,

    pub color: Option<String>,
    pub blur_hash: Option<String>,

    pub author_name: String,
    pub author_url: String,
    pub author_avatar: String,

    pub views: Option<i64>,
    pub downloads: Option<i64>,
    pub likes: Option<i64>,
    pub comments: Option<i64>,

    pub preview_only: bool,
    pub vision_processed: bool,
    pub ai_generated: Option<bool>,

    pub created_at_provider: Option<String>,
    pub downloaded_at: String,

    /// Raw provider JSON for any fields we don't normalize (EXIF, location,
    /// collections, etc.). Stored so the data is never lost.
    pub source_data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewImage {
    pub source: String,
    pub source_id: String,
    pub kind: String,
    pub source_page_url: String,

    pub filename: String,
    pub path: String,
    pub thumb_path: String,
    pub url: String,
    pub urls: Value,

    pub width: i64,
    pub height: i64,
    pub duration_secs: Option<i64>,
    pub file_size: Option<i64>,

    pub query: String,
    pub alt: String,
    pub tags: Vec<String>,

    pub color: Option<String>,
    pub blur_hash: Option<String>,

    pub author_name: String,
    pub author_url: String,
    pub author_avatar: String,

    pub views: Option<i64>,
    pub downloads: Option<i64>,
    pub likes: Option<i64>,
    pub comments: Option<i64>,

    pub preview_only: bool,
    pub ai_generated: Option<bool>,

    pub created_at_provider: Option<String>,

    pub source_data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResult {
    pub deleted: usize,
    pub failed: usize,
}
