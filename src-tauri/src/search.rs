use std::collections::HashMap;
use std::sync::Arc;

use futures_util::future::join_all;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::config::Settings;
use crate::error::Result;
use crate::quota::{QuotaTracker, Source as QuotaSource};

/// Search-time representation of a media item. Carries every field we'll
/// persist after download, plus the raw provider blob so nothing is lost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub source: String,
    pub source_id: String,
    pub kind: String, // "photo" | "video" | "illustration" | "vector"
    pub source_page_url: String,

    /// Preferred download URL: largest sensible quality. For videos, an .mp4.
    pub url: String,
    /// All known URL tiers as a JSON object.
    pub urls: Value,

    pub query: String,
    pub tags: Vec<String>,
    pub alt: String,

    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_secs: Option<i64>,
    pub file_size: Option<i64>,

    pub color: Option<String>,
    pub blur_hash: Option<String>,

    pub author_name: String,
    pub author_url: String,
    pub author_avatar: String,

    pub views: Option<i64>,
    pub downloads: Option<i64>,
    pub likes: Option<i64>,
    pub comments: Option<i64>,

    pub ai_generated: Option<bool>,
    pub created_at_provider: Option<String>,

    pub source_data: Value,
}

impl SearchResult {
    pub fn empty(source: &str, kind: &str, query: &str) -> Self {
        Self {
            source: source.to_string(),
            source_id: String::new(),
            kind: kind.to_string(),
            source_page_url: String::new(),
            url: String::new(),
            urls: json!({}),
            query: query.to_string(),
            tags: Vec::new(),
            alt: String::new(),
            width: None,
            height: None,
            duration_secs: None,
            file_size: None,
            color: None,
            blur_hash: None,
            author_name: String::new(),
            author_url: String::new(),
            author_avatar: String::new(),
            views: None,
            downloads: None,
            likes: None,
            comments: None,
            ai_generated: None,
            created_at_provider: None,
            source_data: Value::Null,
        }
    }
}

// ============================================================================
// Filters — unified across providers; mapped per-provider where supported.
// ============================================================================

/// Provider-agnostic filter spec. Each provider applies what it supports
/// and ignores the rest.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SearchFilters {
    /// "any" (or unset) | "horizontal" | "vertical" | "square"
    pub orientation: Option<String>,

    /// Either a named color (red, orange, yellow, green, turquoise, blue,
    /// lilac, pink, white, gray, black, brown) or a hex like "#1a2b3c".
    /// Pexels accepts named and hex. Pixabay accepts a comma list of named.
    /// Unsplash accepts a fixed named set.
    pub color: Option<String>,

    pub min_width: Option<u32>,
    pub min_height: Option<u32>,

    /// Pixabay categories (e.g. "backgrounds","fashion","nature",…).
    pub category: Option<String>,

    /// "popular" (Pixabay default), "latest", "relevant" (Unsplash default).
    pub order: Option<String>,

    /// Pixabay only: "all" | "photo" | "illustration" | "vector".
    pub image_type: Option<String>,

    /// Pixabay videos only: "all" | "film" | "animation".
    pub video_type: Option<String>,

    /// Pexels only: "large" (≥24MP) | "medium" (≥12MP) | "small" (≥4MP).
    pub size: Option<String>,

    /// Pixabay only: safesearch toggle (default off).
    pub safesearch: Option<bool>,

    /// Pixabay only: editor's choice picks.
    pub editors_choice: Option<bool>,

    /// Post-filter: drop results where ai_generated == true (Pixabay only
    /// reports this; Pexels/Unsplash don't and pass through).
    pub exclude_ai: Option<bool>,

    /// How many items the user wants per source. Translated to per_page +
    /// pages per provider's max page size. None → 1 provider-default page.
    pub count_per_source: Option<u32>,
}

impl SearchFilters {
    fn nonempty<'a>(&self, opt: &'a Option<String>) -> Option<&'a str> {
        opt.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }

    fn orientation(&self) -> Option<&str> {
        let v = self.nonempty(&self.orientation)?;
        if v.eq_ignore_ascii_case("any") {
            None
        } else {
            Some(v)
        }
    }
}

// Provider-specific page-size caps.
const PIXABAY_PHOTO_MAX_PER_PAGE: u32 = 200;
const PIXABAY_VIDEO_MAX_PER_PAGE: u32 = 200;
const PEXELS_MAX_PER_PAGE: u32 = 80;
const UNSPLASH_MAX_PER_PAGE: u32 = 30;

#[derive(Debug, Clone, Copy)]
pub struct UnsplashSearchOptions {
    pub page: u32,
    pub per_page: u32,
    pub fetch_details: bool,
}

fn paginate(count: u32, max_per_page: u32) -> Vec<(u32, u32)> {
    if count == 0 {
        return Vec::new();
    }
    let per_page = count.min(max_per_page).max(3);
    let mut pages = Vec::new();
    let mut left = count;
    let mut page = 1u32;
    while left > 0 {
        let take = left.min(per_page);
        pages.push((page, take));
        left = left.saturating_sub(take);
        page += 1;
        if page > 50 {
            break; // safety
        }
    }
    pages
}

// ============================================================================
// Pixabay photos
// ============================================================================

pub async fn search_pixabay(
    client: &Client,
    key: &str,
    query: &str,
    filters: &SearchFilters,
    page: u32,
    per_page: u32,
    tracker: Option<&Arc<QuotaTracker>>,
) -> Result<Vec<SearchResult>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let mut q: Vec<(String, String)> = vec![
        ("key".into(), key.to_string()),
        ("q".into(), query.to_string()),
        ("per_page".into(), per_page.to_string()),
        ("page".into(), page.to_string()),
    ];

    // image_type defaults to photo here (videos go via the video endpoint).
    let image_type = filters
        .nonempty(&filters.image_type)
        .filter(|s| !s.eq_ignore_ascii_case("any"))
        .unwrap_or("photo");
    q.push(("image_type".into(), image_type.to_string()));

    if let Some(o) = filters.orientation() {
        // Pixabay calls them "horizontal" / "vertical" / "all". We accept
        // "square" for UI parity but Pixabay has no square filter — skip.
        if o.eq_ignore_ascii_case("horizontal") || o.eq_ignore_ascii_case("vertical") {
            q.push(("orientation".into(), o.to_lowercase()));
        }
    }
    if let Some(c) = filters.nonempty(&filters.category) {
        q.push(("category".into(), c.to_string()));
    }
    if let Some(c) = filters.nonempty(&filters.color) {
        if let Some(named) = pixabay_color(c) {
            q.push(("colors".into(), named.into()));
        }
    }
    if let Some(min_w) = filters.min_width {
        if min_w > 0 {
            q.push(("min_width".into(), min_w.to_string()));
        }
    }
    if let Some(min_h) = filters.min_height {
        if min_h > 0 {
            q.push(("min_height".into(), min_h.to_string()));
        }
    }
    if let Some(o) = filters.nonempty(&filters.order) {
        if o.eq_ignore_ascii_case("popular") || o.eq_ignore_ascii_case("latest") {
            q.push(("order".into(), o.to_lowercase()));
        }
    }
    if filters.editors_choice.unwrap_or(false) {
        q.push(("editors_choice".into(), "true".into()));
    }
    q.push((
        "safesearch".into(),
        if filters.safesearch.unwrap_or(true) {
            "true"
        } else {
            "false"
        }
        .into(),
    ));

    let resp = client
        .get("https://pixabay.com/api/")
        .query(&q)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;
    if let Some(t) = tracker {
        t.record(QuotaSource::Pixabay, &resp);
    }
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: Value = resp.json().await?;
    let hits = body
        .get("hits")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut results: Vec<SearchResult> = hits
        .into_iter()
        .map(|hit| pixabay_photo_from_hit(hit, query))
        .collect();
    if filters.exclude_ai.unwrap_or(false) {
        results.retain(|r| !r.ai_generated.unwrap_or(false));
    }
    Ok(results)
}

fn pixabay_photo_from_hit(hit: Value, query: &str) -> SearchResult {
    let mut r = SearchResult::empty("Pixabay", "photo", query);
    r.source_id = hit
        .get("id")
        .and_then(|v| v.as_i64())
        .map(|n| n.to_string())
        .unwrap_or_default();
    r.source_page_url = string_field(&hit, "pageURL");
    r.url = string_field(&hit, "largeImageURL");
    r.urls = json!({
        "preview": hit.get("previewURL").cloned().unwrap_or(Value::Null),
        "webformat": hit.get("webformatURL").cloned().unwrap_or(Value::Null),
        "large": hit.get("largeImageURL").cloned().unwrap_or(Value::Null),
    });
    r.tags = string_field(&hit, "tags")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    r.alt = string_field(&hit, "name");
    r.width = hit.get("imageWidth").and_then(|v| v.as_i64());
    r.height = hit.get("imageHeight").and_then(|v| v.as_i64());
    r.file_size = hit.get("imageSize").and_then(|v| v.as_i64());
    r.author_name = string_field(&hit, "user");
    r.author_url = string_field(&hit, "userURL");
    r.author_avatar = string_field(&hit, "userImageURL");
    r.views = hit.get("views").and_then(|v| v.as_i64());
    r.downloads = hit.get("downloads").and_then(|v| v.as_i64());
    r.likes = hit.get("likes").and_then(|v| v.as_i64());
    r.comments = hit.get("comments").and_then(|v| v.as_i64());
    r.ai_generated = hit.get("isAiGenerated").and_then(|v| v.as_bool());
    r.source_data = hit;
    r
}

fn pixabay_color(s: &str) -> Option<&'static str> {
    match s.trim().to_ascii_lowercase().as_str() {
        "grayscale" => Some("grayscale"),
        "transparent" => Some("transparent"),
        "red" => Some("red"),
        "orange" => Some("orange"),
        "yellow" => Some("yellow"),
        "green" => Some("green"),
        "turquoise" => Some("turquoise"),
        "blue" => Some("blue"),
        "lilac" | "purple" | "violet" => Some("lilac"),
        "pink" => Some("pink"),
        "white" => Some("white"),
        "gray" | "grey" => Some("gray"),
        "black" => Some("black"),
        "brown" => Some("brown"),
        _ => None,
    }
}

// ============================================================================
// Pixabay videos
// ============================================================================

pub async fn search_pixabay_videos(
    client: &Client,
    key: &str,
    query: &str,
    filters: &SearchFilters,
    page: u32,
    per_page: u32,
    tracker: Option<&Arc<QuotaTracker>>,
) -> Result<Vec<SearchResult>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let mut q: Vec<(String, String)> = vec![
        ("key".into(), key.to_string()),
        ("q".into(), query.to_string()),
        ("per_page".into(), per_page.to_string()),
        ("page".into(), page.to_string()),
    ];

    if let Some(v) = filters.nonempty(&filters.video_type) {
        if !v.eq_ignore_ascii_case("any") {
            q.push(("video_type".into(), v.to_lowercase()));
        }
    }
    if let Some(c) = filters.nonempty(&filters.category) {
        q.push(("category".into(), c.to_string()));
    }
    if let Some(min_w) = filters.min_width {
        if min_w > 0 {
            q.push(("min_width".into(), min_w.to_string()));
        }
    }
    if let Some(min_h) = filters.min_height {
        if min_h > 0 {
            q.push(("min_height".into(), min_h.to_string()));
        }
    }
    if let Some(o) = filters.nonempty(&filters.order) {
        if o.eq_ignore_ascii_case("popular") || o.eq_ignore_ascii_case("latest") {
            q.push(("order".into(), o.to_lowercase()));
        }
    }
    if filters.editors_choice.unwrap_or(false) {
        q.push(("editors_choice".into(), "true".into()));
    }
    q.push((
        "safesearch".into(),
        if filters.safesearch.unwrap_or(true) {
            "true"
        } else {
            "false"
        }
        .into(),
    ));

    let resp = client
        .get("https://pixabay.com/api/videos/")
        .query(&q)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;
    if let Some(t) = tracker {
        t.record(QuotaSource::Pixabay, &resp);
    }
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: Value = resp.json().await?;
    let hits = body
        .get("hits")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut results: Vec<SearchResult> = hits
        .into_iter()
        .map(|hit| pixabay_video_from_hit(hit, query))
        .collect();
    if filters.exclude_ai.unwrap_or(false) {
        results.retain(|r| !r.ai_generated.unwrap_or(false));
    }
    Ok(results)
}

fn pixabay_video_from_hit(hit: Value, query: &str) -> SearchResult {
    let mut r = SearchResult::empty("Pixabay", "video", query);
    r.source_id = hit
        .get("id")
        .and_then(|v| v.as_i64())
        .map(|n| n.to_string())
        .unwrap_or_default();
    r.source_page_url = string_field(&hit, "pageURL");
    r.alt = string_field(&hit, "name");
    r.tags = string_field(&hit, "tags")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    r.duration_secs = hit.get("duration").and_then(|v| v.as_i64());
    r.author_name = string_field(&hit, "user");
    r.author_url = string_field(&hit, "userURL");
    r.author_avatar = string_field(&hit, "userImageURL");
    r.views = hit.get("views").and_then(|v| v.as_i64());
    r.downloads = hit.get("downloads").and_then(|v| v.as_i64());
    r.likes = hit.get("likes").and_then(|v| v.as_i64());
    r.comments = hit.get("comments").and_then(|v| v.as_i64());
    r.ai_generated = hit.get("isAiGenerated").and_then(|v| v.as_bool());

    // Pick best video variant; gather tier urls
    let mut urls = serde_json::Map::new();
    let videos = hit.get("videos").cloned().unwrap_or(Value::Null);
    if let Some(obj) = videos.as_object() {
        for (tier, value) in obj {
            if let Some(url) = value.get("url").and_then(|v| v.as_str()) {
                urls.insert(tier.clone(), Value::String(url.to_string()));
            }
        }
        for tier in ["large", "medium", "small", "tiny"] {
            if let Some(v) = obj.get(tier) {
                if let Some(url) = v.get("url").and_then(|s| s.as_str()) {
                    if !url.is_empty() {
                        r.url = url.to_string();
                        r.width = v.get("width").and_then(|n| n.as_i64());
                        r.height = v.get("height").and_then(|n| n.as_i64());
                        r.file_size = v.get("size").and_then(|n| n.as_i64());
                        if r.url.is_empty() {
                            continue;
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        // Poster thumbnail: pick from the tier we used or any tier
        for tier in ["large", "medium", "small", "tiny"] {
            if let Some(thumb) = obj
                .get(tier)
                .and_then(|v| v.get("thumbnail"))
                .and_then(|v| v.as_str())
            {
                if !thumb.is_empty() {
                    urls.insert("poster".to_string(), Value::String(thumb.to_string()));
                    break;
                }
            }
        }
    }
    r.urls = Value::Object(urls);
    r.source_data = hit;
    r
}

// ============================================================================
// Pexels photos
// ============================================================================

pub async fn search_pexels(
    client: &Client,
    key: &str,
    query: &str,
    filters: &SearchFilters,
    page: u32,
    per_page: u32,
    tracker: Option<&Arc<QuotaTracker>>,
) -> Result<Vec<SearchResult>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let mut q: Vec<(String, String)> = Vec::new();
    q.push(("query".into(), query.to_string()));
    q.push(("per_page".into(), per_page.to_string()));
    q.push(("page".into(), page.to_string()));

    if let Some(o) = filters.orientation() {
        let lower = o.to_ascii_lowercase();
        let mapped: Option<&str> = match lower.as_str() {
            "horizontal" | "landscape" => Some("landscape"),
            "vertical" | "portrait" => Some("portrait"),
            "square" => Some("square"),
            _ => None,
        };
        if let Some(m) = mapped {
            q.push(("orientation".into(), m.to_string()));
        }
    }
    if let Some(s) = filters.nonempty(&filters.size) {
        let s = s.to_ascii_lowercase();
        if matches!(s.as_str(), "large" | "medium" | "small") {
            q.push(("size".into(), s));
        }
    }
    if let Some(c) = filters.nonempty(&filters.color) {
        q.push(("color".into(), c.to_lowercase()));
    }

    let resp = client
        .get("https://api.pexels.com/v1/search")
        .header("Authorization", key)
        .query(&q)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;
    if let Some(t) = tracker {
        t.record(QuotaSource::Pexels, &resp);
    }
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: Value = resp.json().await?;
    let photos = body
        .get("photos")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut results: Vec<SearchResult> = photos
        .into_iter()
        .map(|p| pexels_photo_from(p, query))
        .collect();
    if let Some(min_w) = filters.min_width {
        results.retain(|r| r.width.map(|w| w as u32 >= min_w).unwrap_or(true));
    }
    if let Some(min_h) = filters.min_height {
        results.retain(|r| r.height.map(|h| h as u32 >= min_h).unwrap_or(true));
    }
    Ok(results)
}

fn pexels_photo_from(p: Value, query: &str) -> SearchResult {
    let mut r = SearchResult::empty("Pexels", "photo", query);
    r.source_id = p
        .get("id")
        .and_then(|v| v.as_i64())
        .map(|n| n.to_string())
        .unwrap_or_default();
    r.source_page_url = string_field(&p, "url");
    r.alt = string_field(&p, "alt");
    r.color = optional_string(&p, "avg_color");
    r.width = p.get("width").and_then(|v| v.as_i64());
    r.height = p.get("height").and_then(|v| v.as_i64());
    r.author_name = string_field(&p, "photographer");
    r.author_url = string_field(&p, "photographer_url");
    r.tags = vec![query.to_string()];

    let src = p.get("src").cloned().unwrap_or(Value::Null);
    if let Some(obj) = src.as_object() {
        let mut urls = serde_json::Map::new();
        for (tier, value) in obj {
            urls.insert(tier.clone(), value.clone());
        }
        r.urls = Value::Object(urls);
        // Prefer large2x as the main download, fall back to original/large
        for tier in ["large2x", "original", "large", "medium"] {
            if let Some(url) = obj.get(tier).and_then(|v| v.as_str()) {
                if !url.is_empty() {
                    r.url = url.to_string();
                    break;
                }
            }
        }
    }
    r.source_data = p;
    r
}

// ============================================================================
// Pexels videos
// ============================================================================

pub async fn search_pexels_videos(
    client: &Client,
    key: &str,
    query: &str,
    filters: &SearchFilters,
    page: u32,
    per_page: u32,
    tracker: Option<&Arc<QuotaTracker>>,
) -> Result<Vec<SearchResult>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let mut q: Vec<(String, String)> = Vec::new();
    q.push(("query".into(), query.to_string()));
    q.push(("per_page".into(), per_page.to_string()));
    q.push(("page".into(), page.to_string()));

    if let Some(o) = filters.orientation() {
        let lower = o.to_ascii_lowercase();
        let mapped: &str = match lower.as_str() {
            "horizontal" => "landscape",
            "vertical" => "portrait",
            "landscape" | "portrait" | "square" => lower.as_str(),
            _ => "landscape",
        };
        q.push(("orientation".into(), mapped.to_string()));
    }
    if let Some(s) = filters.nonempty(&filters.size) {
        let s = s.to_ascii_lowercase();
        if matches!(s.as_str(), "large" | "medium" | "small") {
            q.push(("size".into(), s));
        }
    }
    if let Some(min_w) = filters.min_width {
        if min_w > 0 {
            q.push(("min_width".into(), min_w.to_string()));
        }
    }
    if let Some(min_h) = filters.min_height {
        if min_h > 0 {
            q.push(("min_height".into(), min_h.to_string()));
        }
    }

    let resp = client
        .get("https://api.pexels.com/videos/search")
        .header("Authorization", key)
        .query(&q)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;
    if let Some(t) = tracker {
        t.record(QuotaSource::Pexels, &resp);
    }
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: Value = resp.json().await?;
    let videos = body
        .get("videos")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(videos
        .into_iter()
        .map(|v| pexels_video_from(v, query))
        .collect())
}

fn pexels_video_from(v: Value, query: &str) -> SearchResult {
    let mut r = SearchResult::empty("Pexels", "video", query);
    r.source_id = v
        .get("id")
        .and_then(|n| n.as_i64())
        .map(|n| n.to_string())
        .unwrap_or_default();
    r.source_page_url = string_field(&v, "url");
    r.duration_secs = v.get("duration").and_then(|n| n.as_i64());
    r.width = v.get("width").and_then(|n| n.as_i64());
    r.height = v.get("height").and_then(|n| n.as_i64());
    r.tags = vec![query.to_string()];

    if let Some(user) = v.get("user") {
        r.author_name = string_field(user, "name");
        r.author_url = string_field(user, "url");
    }

    // Build url tiers from video_files. Prefer hd with mp4, then sd, skip hls.
    let mut tiers: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut best_file: Option<Value> = None;
    let mut best_score: i64 = -1;
    if let Some(files) = v.get("video_files").and_then(|f| f.as_array()) {
        for file in files {
            let quality = file.get("quality").and_then(|q| q.as_str()).unwrap_or("");
            let file_type = file.get("file_type").and_then(|q| q.as_str()).unwrap_or("");
            let link = file.get("link").and_then(|q| q.as_str()).unwrap_or("");
            if link.is_empty() {
                continue;
            }
            let key = if !quality.is_empty() {
                format!("{quality}_{file_type}")
            } else {
                file_type.to_string()
            };
            tiers.insert(key, Value::String(link.to_string()));
            if file_type.contains("hls") {
                continue;
            }
            let score = match quality {
                "hd" => 100,
                "sd" => 50,
                _ => 10,
            };
            if score > best_score {
                best_score = score;
                best_file = Some(file.clone());
            }
        }
    }
    if let Some(file) = best_file {
        r.url = file
            .get("link")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        r.width = file.get("width").and_then(|n| n.as_i64()).or(r.width);
        r.height = file.get("height").and_then(|n| n.as_i64()).or(r.height);
    }
    // Poster thumbnail
    if let Some(image) = v.get("image").and_then(|s| s.as_str()) {
        if !image.is_empty() {
            tiers.insert("poster".to_string(), Value::String(image.to_string()));
        }
    } else if let Some(pictures) = v.get("video_pictures").and_then(|p| p.as_array()) {
        if let Some(first) = pictures.first() {
            if let Some(pic) = first.get("picture").and_then(|s| s.as_str()) {
                if !pic.is_empty() {
                    tiers.insert("poster".to_string(), Value::String(pic.to_string()));
                }
            }
        }
    }
    r.urls = Value::Object(tiers);
    r.source_data = v;
    r
}

// ============================================================================
// Unsplash photos (no video API)
// ============================================================================

pub async fn search_unsplash(
    client: &Client,
    key: &str,
    query: &str,
    filters: &SearchFilters,
    tracker: Option<&Arc<QuotaTracker>>,
    options: UnsplashSearchOptions,
) -> Result<Vec<SearchResult>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let auth = format!("Client-ID {key}");
    let mut q: Vec<(String, String)> = Vec::new();
    q.push(("query".into(), query.to_string()));
    q.push(("per_page".into(), options.per_page.to_string()));
    q.push(("page".into(), options.page.to_string()));

    if let Some(o) = filters.orientation() {
        let mapped = match o.to_ascii_lowercase().as_str() {
            "horizontal" | "landscape" => Some("landscape"),
            "vertical" | "portrait" => Some("portrait"),
            "square" | "squarish" => Some("squarish"),
            _ => None,
        };
        if let Some(m) = mapped {
            q.push(("orientation".into(), m.to_string()));
        }
    }
    if let Some(c) = filters.nonempty(&filters.color) {
        if let Some(named) = unsplash_color(c) {
            q.push(("color".into(), named.into()));
        }
    }
    if let Some(o) = filters.nonempty(&filters.order) {
        let ob = match o.to_ascii_lowercase().as_str() {
            "latest" => "latest",
            "relevant" | "popular" => "relevant",
            _ => "relevant",
        };
        q.push(("order_by".into(), ob.to_string()));
    }

    let resp = client
        .get("https://api.unsplash.com/search/photos")
        .header("Authorization", &auth)
        .query(&q)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;
    if let Some(t) = tracker {
        t.record(QuotaSource::Unsplash, &resp);
    }
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: Value = resp.json().await?;
    let summaries = body
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Optionally fetch each photo in detail in parallel for richer metadata.
    // Each detail call counts against Unsplash's per-hour quota — when the
    // user disables this (or a budget threshold trips), we fall back to the
    // search summary which already has enough for download + display.
    if !options.fetch_details {
        let mut results: Vec<SearchResult> = summaries
            .into_iter()
            .map(|p| unsplash_photo_from(p, query))
            .collect();
        if let Some(min_w) = filters.min_width {
            results.retain(|r| r.width.map(|w| w as u32 >= min_w).unwrap_or(true));
        }
        if let Some(min_h) = filters.min_height {
            results.retain(|r| r.height.map(|h| h as u32 >= min_h).unwrap_or(true));
        }
        return Ok(results);
    }

    let detail_futs = summaries.into_iter().map(|p| {
        let client = client.clone();
        let auth = auth.clone();
        let query = query.to_string();
        let tracker_opt = tracker.cloned();
        async move {
            let id = p
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                return None;
            }
            let url = format!("https://api.unsplash.com/photos/{id}");
            let resp = client
                .get(&url)
                .header("Authorization", &auth)
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .ok()?;
            if let Some(t) = tracker_opt.as_ref() {
                t.record(QuotaSource::Unsplash, &resp);
            }
            if !resp.status().is_success() {
                return None;
            }
            let full: Value = resp.json().await.ok()?;
            Some(unsplash_photo_from(full, &query))
        }
    });

    let detail_results = join_all(detail_futs).await;
    let mut results: Vec<SearchResult> = detail_results.into_iter().flatten().collect();
    if let Some(min_w) = filters.min_width {
        results.retain(|r| r.width.map(|w| w as u32 >= min_w).unwrap_or(true));
    }
    if let Some(min_h) = filters.min_height {
        results.retain(|r| r.height.map(|h| h as u32 >= min_h).unwrap_or(true));
    }
    Ok(results)
}

fn unsplash_photo_from(p: Value, query: &str) -> SearchResult {
    let mut r = SearchResult::empty("Unsplash", "photo", query);
    r.source_id = string_field(&p, "id");
    r.source_page_url = p
        .get("links")
        .and_then(|l| l.get("html"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    r.alt = p
        .get("alt_description")
        .and_then(|v| v.as_str())
        .or_else(|| p.get("description").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    r.color = optional_string(&p, "color");
    r.blur_hash = optional_string(&p, "blur_hash");
    r.width = p.get("width").and_then(|v| v.as_i64());
    r.height = p.get("height").and_then(|v| v.as_i64());
    r.created_at_provider = optional_string(&p, "created_at");
    r.likes = p.get("likes").and_then(|v| v.as_i64());
    r.downloads = p.get("downloads").and_then(|v| v.as_i64());
    r.views = p.get("views").and_then(|v| v.as_i64());

    if let Some(user) = p.get("user") {
        r.author_name = string_field(user, "name");
        r.author_url = user
            .get("links")
            .and_then(|l| l.get("html"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        r.author_avatar = user
            .get("profile_image")
            .and_then(|p| p.get("medium"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }

    if let Some(tags) = p.get("tags").and_then(|v| v.as_array()) {
        r.tags = tags
            .iter()
            .filter_map(|t| {
                t.get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .filter(|s| !s.is_empty())
            .collect();
    }

    if let Some(urls) = p.get("urls").and_then(|v| v.as_object()) {
        let mut copy = serde_json::Map::new();
        for (k, v) in urls {
            copy.insert(k.clone(), v.clone());
        }
        r.urls = Value::Object(copy);
        for tier in ["full", "raw", "regular", "small"] {
            if let Some(url) = urls.get(tier).and_then(|v| v.as_str()) {
                if !url.is_empty() {
                    r.url = url.to_string();
                    break;
                }
            }
        }
    }
    r.source_data = p;
    r
}

fn unsplash_color(s: &str) -> Option<&'static str> {
    match s.trim().to_ascii_lowercase().as_str() {
        "black_and_white" | "bw" | "grayscale" => Some("black_and_white"),
        "black" => Some("black"),
        "white" => Some("white"),
        "yellow" => Some("yellow"),
        "orange" => Some("orange"),
        "red" => Some("red"),
        "purple" | "violet" | "lilac" => Some("purple"),
        "magenta" | "pink" => Some("magenta"),
        "green" => Some("green"),
        "teal" | "turquoise" => Some("teal"),
        "blue" => Some("blue"),
        _ => None,
    }
}

// ============================================================================
// Combined search
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Photos,
    Videos,
    Both,
}

impl Kind {
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "video" | "videos" => Kind::Videos,
            "all" | "both" | "any" => Kind::Both,
            _ => Kind::Photos,
        }
    }
}

/// Source-pages map kept for backwards-compat with the existing REST + Tauri
/// surface. The `count_per_source` field on `SearchFilters` overrides this
/// when set; otherwise each entry's value is treated as a page count at each
/// provider's max page size.
pub async fn search_all(
    client: &Client,
    settings: Arc<RwLock<Settings>>,
    query: String,
    source_pages: HashMap<String, u32>,
    kind: Kind,
    filters: SearchFilters,
    tracker: Option<Arc<QuotaTracker>>,
) -> Result<Vec<SearchResult>> {
    let snapshot = settings.read().await.clone();

    let want_photos = matches!(kind, Kind::Photos | Kind::Both);
    let want_videos = matches!(kind, Kind::Videos | Kind::Both);

    let count_override = filters.count_per_source;

    let mut tasks: Vec<tokio::task::JoinHandle<Result<Vec<SearchResult>>>> = Vec::new();

    // Heuristic for skipping the per-result Unsplash detail fetch: when the
    // user is harvesting a large number of items, the extra round-trip per
    // result blows hourly free-tier quota. Above the threshold we use only
    // the search-summary fields, which still have everything we need to
    // download + render. Tunable in Settings.
    let unsplash_detail_threshold: u32 = snapshot.unsplash_detail_threshold;

    if let Some(&pages_arg) = source_pages.get("pixabay") {
        if pages_arg > 0 {
            let count = count_override
                .unwrap_or(pages_arg.saturating_mul(PIXABAY_PHOTO_MAX_PER_PAGE))
                .max(1);
            if want_photos {
                for (page, per_page) in paginate(count, PIXABAY_PHOTO_MAX_PER_PAGE) {
                    let client = client.clone();
                    let key = snapshot.pixabay_key.clone();
                    let q = query.clone();
                    let f = filters.clone();
                    let tr = tracker.clone();
                    tasks.push(tokio::spawn(async move {
                        search_pixabay(&client, &key, &q, &f, page, per_page, tr.as_ref()).await
                    }));
                }
            }
            if want_videos {
                for (page, per_page) in paginate(count, PIXABAY_VIDEO_MAX_PER_PAGE) {
                    let client = client.clone();
                    let key = snapshot.pixabay_key.clone();
                    let q = query.clone();
                    let f = filters.clone();
                    let tr = tracker.clone();
                    tasks.push(tokio::spawn(async move {
                        search_pixabay_videos(&client, &key, &q, &f, page, per_page, tr.as_ref())
                            .await
                    }));
                }
            }
        }
    }
    if let Some(&pages_arg) = source_pages.get("pexels") {
        if pages_arg > 0 {
            let count = count_override
                .unwrap_or(pages_arg.saturating_mul(PEXELS_MAX_PER_PAGE))
                .max(1);
            if want_photos {
                for (page, per_page) in paginate(count, PEXELS_MAX_PER_PAGE) {
                    let client = client.clone();
                    let key = snapshot.pexels_key.clone();
                    let q = query.clone();
                    let f = filters.clone();
                    let tr = tracker.clone();
                    tasks.push(tokio::spawn(async move {
                        search_pexels(&client, &key, &q, &f, page, per_page, tr.as_ref()).await
                    }));
                }
            }
            if want_videos {
                for (page, per_page) in paginate(count, PEXELS_MAX_PER_PAGE) {
                    let client = client.clone();
                    let key = snapshot.pexels_key.clone();
                    let q = query.clone();
                    let f = filters.clone();
                    let tr = tracker.clone();
                    tasks.push(tokio::spawn(async move {
                        search_pexels_videos(&client, &key, &q, &f, page, per_page, tr.as_ref())
                            .await
                    }));
                }
            }
        }
    }
    if let Some(&pages_arg) = source_pages.get("unsplash") {
        if pages_arg > 0 {
            let count = count_override
                .unwrap_or(pages_arg.saturating_mul(UNSPLASH_MAX_PER_PAGE))
                .max(1);
            let fetch_details = count <= unsplash_detail_threshold;
            if want_photos {
                for (page, per_page) in paginate(count, UNSPLASH_MAX_PER_PAGE) {
                    let client = client.clone();
                    let key = snapshot.unsplash_key.clone();
                    let q = query.clone();
                    let f = filters.clone();
                    let tr = tracker.clone();
                    tasks.push(tokio::spawn(async move {
                        search_unsplash(
                            &client,
                            &key,
                            &q,
                            &f,
                            tr.as_ref(),
                            UnsplashSearchOptions {
                                page,
                                per_page,
                                fetch_details,
                            },
                        )
                        .await
                    }));
                }
            }
            // No Unsplash video API
        }
    }

    let mut combined = Vec::new();
    for handle in tasks {
        if let Ok(Ok(mut results)) = handle.await {
            combined.append(&mut results);
        }
    }
    Ok(combined)
}

// ============================================================================
// Helpers
// ============================================================================

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}
