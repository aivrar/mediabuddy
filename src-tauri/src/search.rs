use std::collections::HashMap;
use std::sync::Arc;

use futures_util::future::join_all;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::config::Settings;
use crate::error::Result;

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
// Pixabay photos
// ============================================================================

pub async fn search_pixabay(
    client: &Client,
    key: &str,
    query: &str,
    page: u32,
) -> Result<Vec<SearchResult>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let resp = client
        .get("https://pixabay.com/api/")
        .query(&[
            ("key", key),
            ("q", query),
            ("per_page", "200"),
            ("page", &page.to_string()),
            ("image_type", "photo"),
            ("safesearch", "true"),
        ])
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: Value = resp.json().await?;
    let hits = body
        .get("hits")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(hits
        .into_iter()
        .map(|hit| pixabay_photo_from_hit(hit, query))
        .collect())
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

// ============================================================================
// Pixabay videos
// ============================================================================

pub async fn search_pixabay_videos(
    client: &Client,
    key: &str,
    query: &str,
    page: u32,
) -> Result<Vec<SearchResult>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let resp = client
        .get("https://pixabay.com/api/videos/")
        .query(&[
            ("key", key),
            ("q", query),
            ("per_page", "100"),
            ("page", &page.to_string()),
            ("safesearch", "true"),
        ])
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: Value = resp.json().await?;
    let hits = body
        .get("hits")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(hits
        .into_iter()
        .map(|hit| pixabay_video_from_hit(hit, query))
        .collect())
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
            if let Some(thumb) = obj.get(tier).and_then(|v| v.get("thumbnail")).and_then(|v| v.as_str()) {
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
    page: u32,
) -> Result<Vec<SearchResult>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let resp = client
        .get("https://api.pexels.com/v1/search")
        .header("Authorization", key)
        .query(&[
            ("query", query),
            ("per_page", "80"),
            ("page", &page.to_string()),
        ])
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: Value = resp.json().await?;
    let photos = body
        .get("photos")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(photos
        .into_iter()
        .map(|p| pexels_photo_from(p, query))
        .collect())
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
    page: u32,
) -> Result<Vec<SearchResult>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let resp = client
        .get("https://api.pexels.com/videos/search")
        .header("Authorization", key)
        .query(&[
            ("query", query),
            ("per_page", "80"),
            ("page", &page.to_string()),
        ])
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;
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
    let mut best_link: Option<String> = None;
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
                best_link = Some(link.to_string());
            }
        }
    }
    if let Some(link) = best_link {
        r.url = link;
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
    page: u32,
) -> Result<Vec<SearchResult>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let auth = format!("Client-ID {key}");
    let resp = client
        .get("https://api.unsplash.com/search/photos")
        .header("Authorization", &auth)
        .query(&[
            ("query", query),
            ("per_page", "30"),
            ("page", &page.to_string()),
        ])
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: Value = resp.json().await?;
    let summaries = body
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Fetch each photo in detail in parallel for richer metadata.
    let detail_futs = summaries.into_iter().map(|p| {
        let client = client.clone();
        let auth = auth.clone();
        let query = query.to_string();
        async move {
            let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
            if !resp.status().is_success() {
                return None;
            }
            let full: Value = resp.json().await.ok()?;
            Some(unsplash_photo_from(full, &query))
        }
    });

    let detail_results = join_all(detail_futs).await;
    Ok(detail_results.into_iter().flatten().collect())
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
            .filter_map(|t| t.get("title").and_then(|v| v.as_str()).map(|s| s.to_string()))
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

pub async fn search_all(
    client: &Client,
    settings: Arc<RwLock<Settings>>,
    query: String,
    source_pages: HashMap<String, u32>,
    kind: Kind,
) -> Result<Vec<SearchResult>> {
    let snapshot = settings.read().await.clone();

    let want_photos = matches!(kind, Kind::Photos | Kind::Both);
    let want_videos = matches!(kind, Kind::Videos | Kind::Both);

    let mut tasks: Vec<tokio::task::JoinHandle<Result<Vec<SearchResult>>>> = Vec::new();

    if let Some(&pages) = source_pages.get("pixabay") {
        for page in 1..=pages {
            if want_photos {
                let client = client.clone();
                let key = snapshot.pixabay_key.clone();
                let q = query.clone();
                tasks.push(tokio::spawn(async move {
                    search_pixabay(&client, &key, &q, page).await
                }));
            }
            if want_videos {
                let client = client.clone();
                let key = snapshot.pixabay_key.clone();
                let q = query.clone();
                tasks.push(tokio::spawn(async move {
                    search_pixabay_videos(&client, &key, &q, page).await
                }));
            }
        }
    }
    if let Some(&pages) = source_pages.get("pexels") {
        for page in 1..=pages {
            if want_photos {
                let client = client.clone();
                let key = snapshot.pexels_key.clone();
                let q = query.clone();
                tasks.push(tokio::spawn(async move {
                    search_pexels(&client, &key, &q, page).await
                }));
            }
            if want_videos {
                let client = client.clone();
                let key = snapshot.pexels_key.clone();
                let q = query.clone();
                tasks.push(tokio::spawn(async move {
                    search_pexels_videos(&client, &key, &q, page).await
                }));
            }
        }
    }
    if let Some(&pages) = source_pages.get("unsplash") {
        for page in 1..=pages {
            if want_photos {
                let client = client.clone();
                let key = snapshot.unsplash_key.clone();
                let q = query.clone();
                tasks.push(tokio::spawn(async move {
                    search_unsplash(&client, &key, &q, page).await
                }));
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
