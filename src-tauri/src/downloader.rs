use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, GenericImageView, ImageEncoder};
use reqwest::Client;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::error::{AppError, Result};
use crate::image_manager::ImageManager;
use crate::search::SearchResult;
use crate::types::{Image, NewImage};

const THUMB_SIZE: u32 = 300;
const THUMB_QUALITY: u8 = 85;

/// Maximum redirect hops to follow for a caller-supplied media URL. Each hop
/// is re-validated against the SSRF guard before connecting.
const MAX_REDIRECTS: usize = 5;
/// Upper bound on a buffered photo/poster download (guards against
/// decompression-bomb / oversized-body OOM).
const MAX_PHOTO_BYTES: usize = 64 * 1024 * 1024; // 64 MiB
/// Upper bound on a video download. Videos are streamed straight to disk,
/// so this caps disk use per file, not memory.
const MAX_VIDEO_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB
/// Hard cap on download fan-out, to keep an attacker-supplied `concurrency`
/// from tripping `Semaphore::new`'s permit-count assertion (whole-process
/// abort under `panic = "abort"`).
const MAX_DOWNLOAD_CONCURRENCY: usize = 64;

/// Suffix for a file that is still being written. A download only gets its
/// real name once every byte is on disk, so a crash or a forced quit can
/// never leave a truncated file where the library expects a complete one.
const PART_SUFFIX: &str = ".part";

/// Downloads in flight (single items and whole batches both count). Read by
/// the window-close handler so quitting mid-download asks first.
static ACTIVE_DOWNLOADS: AtomicUsize = AtomicUsize::new(0);

pub fn active_downloads() -> usize {
    ACTIVE_DOWNLOADS.load(Ordering::SeqCst)
}

/// Counts one unit of download work for as long as it is alive.
pub struct DownloadGuard(());

impl DownloadGuard {
    pub fn new() -> Self {
        ACTIVE_DOWNLOADS.fetch_add(1, Ordering::SeqCst);
        DownloadGuard(())
    }
}

impl Default for DownloadGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        ACTIVE_DOWNLOADS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_os_string();
    name.push(PART_SUFFIX);
    PathBuf::from(name)
}

/// Write `bytes` to `dest` via a `.part` file and an atomic rename.
async fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = part_path(dest);
    let written = async {
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, dest).await
    }
    .await;
    if let Err(e) = written {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    Ok(())
}

/// Remove `.part` files left behind by a download that was cut off (crash,
/// power loss, forced quit). Called once at startup, before any download
/// can be running. Returns how many were removed.
pub fn cleanup_partial_files(paths: &crate::paths::AppPaths) -> usize {
    let mut removed = 0;
    for dir in [
        &paths.originals,
        &paths.thumbs,
        &paths.videos_originals,
        &paths.videos_thumbs,
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_part = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(PART_SUFFIX))
                .unwrap_or(false);
            if is_part && path.is_file() && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Open `url` with SSRF validation on every redirect hop, and reject a
/// response that already declares itself larger than `max_bytes`. `client`
/// MUST be built with redirect following disabled (see
/// `AppState::download_http`); redirects are followed manually so each
/// `Location` can be re-validated. Returns the response (body not yet read)
/// and its content type.
async fn open_validated(
    client: &Client,
    url: &str,
    max_bytes: u64,
    timeout: Duration,
) -> Result<(reqwest::Response, String)> {
    let mut current =
        reqwest::Url::parse(url).map_err(|e| AppError::other(format!("invalid URL: {e}")))?;
    let mut hops = 0usize;
    loop {
        crate::urlguard::validate_outbound_url(&current)
            .await
            .map_err(AppError::other)?;
        let resp = client.get(current.clone()).timeout(timeout).send().await?;
        let status = resp.status();
        if status.is_redirection() {
            if hops >= MAX_REDIRECTS {
                return Err(AppError::other("too many redirects"));
            }
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| AppError::other("redirect response had no Location header"))?;
            current = current
                .join(location)
                .map_err(|e| AppError::other(format!("invalid redirect target: {e}")))?;
            hops += 1;
            continue;
        }
        if !status.is_success() {
            return Err(AppError::other(format!("HTTP {}", status.as_u16())));
        }
        if let Some(len) = resp.content_length() {
            if len > max_bytes {
                return Err(AppError::other(
                    "response exceeds size cap (Content-Length)",
                ));
            }
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        return Ok((resp, content_type));
    }
}

/// Fetch `url` into memory with a hard cap on the buffered size. Used for
/// photos and posters, which have to be decoded anyway. Returns
/// `(body, content_type)`.
async fn fetch_capped(
    client: &Client,
    url: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<(Bytes, String)> {
    let (mut resp, content_type) = open_validated(client, url, max_bytes as u64, timeout).await?;
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    while let Some(chunk) = resp.chunk().await? {
        let next_len = buf
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| AppError::other("response exceeds size cap"))?;
        if next_len > max_bytes {
            return Err(AppError::other("response exceeds size cap"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok((Bytes::from(buf), content_type))
}

/// Stream a response body to `dest` without holding it in memory. The data
/// goes to a `.part` file that is renamed into place only when complete; on
/// any failure the partial file is removed. Returns the number of bytes.
async fn stream_to_file(mut resp: reqwest::Response, max_bytes: u64, dest: &Path) -> Result<u64> {
    let tmp = part_path(dest);
    let written: Result<u64> = async {
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut total: u64 = 0;
        while let Some(chunk) = resp.chunk().await? {
            total = total.saturating_add(chunk.len() as u64);
            if total > max_bytes {
                return Err(AppError::other("response exceeds size cap"));
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&tmp, dest).await?;
        Ok(total)
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    written
}

fn safe_filename(tags: &[String], width: u32, height: u32, ext: &str) -> String {
    let safe_tags: Vec<String> = tags
        .iter()
        .take(3)
        .map(|t| {
            t.to_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(15)
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .collect();
    let prefix = if safe_tags.is_empty() {
        "media".to_string()
    } else {
        safe_tags.join("_")
    };
    let unique = Uuid::new_v4().simple().to_string();
    format!("{}_{}x{}_{}{}", prefix, width, height, &unique[..8], ext)
}

fn ext_from_content_type(ct: &str) -> &'static str {
    let lower = ct.to_ascii_lowercase();
    if lower.contains("jpeg") || lower.contains("jpg") {
        ".jpg"
    } else if lower.contains("png") {
        ".png"
    } else if lower.contains("webp") {
        ".webp"
    } else if lower.contains("gif") {
        ".gif"
    } else {
        ".jpg"
    }
}

/// File extension for a video. Looks at the URL path first, then the
/// content type, and defaults to `.mp4` — never to an image extension, which
/// would make the saved file unplayable.
fn video_ext(url: &str, ct: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_lowercase();
    let ct = ct.to_ascii_lowercase();
    if path.ends_with(".webm") || ct.contains("webm") {
        ".webm"
    } else if path.ends_with(".mov") || ct.contains("quicktime") {
        ".mov"
    } else if path.ends_with(".m4v") {
        ".m4v"
    } else {
        ".mp4"
    }
}

struct DownloadedMediaParts {
    filename: String,
    path_rel: String,
    thumb_rel: String,
    width: i64,
    height: i64,
    file_size: Option<i64>,
    preview_only: bool,
}

fn build_new_image(result: &SearchResult, parts: DownloadedMediaParts) -> NewImage {
    NewImage {
        source: result.source.clone(),
        source_id: result.source_id.clone(),
        kind: result.kind.clone(),
        source_page_url: result.source_page_url.clone(),
        filename: parts.filename,
        path: parts.path_rel,
        thumb_path: parts.thumb_rel,
        url: result.url.clone(),
        urls: result.urls.clone(),
        width: parts.width,
        height: parts.height,
        duration_secs: result.duration_secs,
        file_size: parts.file_size.or(result.file_size),
        query: result.query.clone(),
        alt: result.alt.clone(),
        tags: result.tags.clone(),
        color: result.color.clone(),
        blur_hash: result.blur_hash.clone(),
        author_name: result.author_name.clone(),
        author_url: result.author_url.clone(),
        author_avatar: result.author_avatar.clone(),
        views: result.views,
        downloads: result.downloads,
        likes: result.likes,
        comments: result.comments,
        preview_only: parts.preview_only,
        ai_generated: result.ai_generated,
        created_at_provider: result.created_at_provider.clone(),
        source_data: result.source_data.clone(),
        caption_source: result.caption_source.clone(),
    }
}

pub async fn download_one(
    client: &Client,
    manager: &Arc<ImageManager>,
    result: &SearchResult,
    preview_only: bool,
) -> Result<Option<Image>> {
    if result.url.is_empty() || manager.is_url_saved(&result.url) {
        return Ok(None);
    }
    let _active = DownloadGuard::new();
    if result.kind == "video" {
        download_video(client, manager, result, preview_only).await
    } else {
        download_photo(client, manager, result, preview_only).await
    }
}

async fn download_photo(
    client: &Client,
    manager: &Arc<ImageManager>,
    result: &SearchResult,
    preview_only: bool,
) -> Result<Option<Image>> {
    let (original_bytes, content_type) = match fetch_capped(
        client,
        &result.url,
        MAX_PHOTO_BYTES,
        Duration::from_secs(60),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("photo download skipped for {}: {}", result.url, e);
            return Ok(None);
        }
    };
    let ext = ext_from_content_type(&content_type);
    let bytes_for_decode = original_bytes.clone();

    let (width, height, thumb_jpeg) =
        tokio::task::spawn_blocking(move || -> Result<(u32, u32, Vec<u8>)> {
            let img = image::load_from_memory(&bytes_for_decode)?;
            let (w, h) = img.dimensions();
            let thumb = img.thumbnail(THUMB_SIZE, THUMB_SIZE).to_rgb8();
            let (tw, th) = thumb.dimensions();
            let mut out = Vec::with_capacity(64 * 1024);
            JpegEncoder::new_with_quality(&mut out, THUMB_QUALITY).write_image(
                thumb.as_raw(),
                tw,
                th,
                ExtendedColorType::Rgb8,
            )?;
            Ok((w, h, out))
        })
        .await
        .map_err(|e| AppError::other(e.to_string()))??;

    let unique = Uuid::new_v4().simple().to_string();
    let thumb_filename = format!("thumb_{}.jpg", &unique[..12]);
    let thumb_full = manager.paths.thumbs.join(&thumb_filename);
    write_atomic(&thumb_full, &thumb_jpeg).await?;
    let thumb_rel = format!("images/thumbs/{}", thumb_filename);

    let (filename, original_rel, file_size, original_full) = if preview_only {
        (thumb_filename, String::new(), None, None)
    } else {
        let filename = safe_filename(&result.tags, width, height, ext);
        let original_full = manager.paths.originals.join(&filename);
        if let Err(e) = write_atomic(&original_full, &original_bytes).await {
            // Don't leave the thumbnail behind with no library row.
            let _ = tokio::fs::remove_file(&thumb_full).await;
            return Err(e);
        }
        let rel = format!("images/originals/{}", filename);
        let size = original_bytes.len() as i64;
        (filename, rel, Some(size), Some(original_full))
    };

    let new = build_new_image(
        result,
        DownloadedMediaParts {
            filename,
            path_rel: original_rel,
            thumb_rel,
            width: width as i64,
            height: height as i64,
            file_size,
            preview_only,
        },
    );
    let manager_for_db = manager.clone();
    let added = tokio::task::spawn_blocking(move || manager_for_db.add_image(&new))
        .await
        .map_err(|e| AppError::other(e.to_string()))?;
    match added {
        Ok(saved) => {
            tracing::info!(
                "download saved: {} {} {}x{} id={}",
                saved.source,
                saved.kind,
                saved.width,
                saved.height,
                saved.id
            );
            Ok(Some(saved))
        }
        Err(e) => {
            // Duplicate URL or DB failure: roll back the files we just wrote
            // so they don't accumulate as orphans on disk.
            let _ = tokio::fs::remove_file(&thumb_full).await;
            if let Some(p) = &original_full {
                let _ = tokio::fs::remove_file(p).await;
            }
            tracing::warn!(
                "add_image failed for {} ({e}); cleaned up partial files",
                result.url
            );
            Ok(None)
        }
    }
}

async fn download_video(
    client: &Client,
    manager: &Arc<ImageManager>,
    result: &SearchResult,
    preview_only: bool,
) -> Result<Option<Image>> {
    let unique = Uuid::new_v4().simple().to_string();
    let width = result.width.unwrap_or(0).max(0) as u32;
    let height = result.height.unwrap_or(0).max(0) as u32;

    // Preview-only keeps just the poster and metadata, so the video file is
    // not fetched at all. Otherwise it is streamed straight to disk.
    let (filename, original_rel, original_full, file_size) = if preview_only {
        let filename = safe_filename(&result.tags, width, height, video_ext(&result.url, ""));
        (filename, String::new(), None, result.file_size)
    } else {
        let (resp, content_type) = match open_validated(
            client,
            &result.url,
            MAX_VIDEO_BYTES,
            Duration::from_secs(300),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("video download skipped for {}: {}", result.url, e);
                return Ok(None);
            }
        };
        let filename = safe_filename(
            &result.tags,
            width,
            height,
            video_ext(&result.url, &content_type),
        );
        let original_full = manager.paths.videos_originals.join(&filename);
        let size = match stream_to_file(resp, MAX_VIDEO_BYTES, &original_full).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("video download failed for {}: {}", result.url, e);
                return Ok(None);
            }
        };
        (
            filename.clone(),
            format!("videos/originals/{}", filename),
            Some(original_full),
            Some(size as i64),
        )
    };

    // Poster thumbnail: download from urls.poster if present
    let poster_url = result
        .urls
        .get("poster")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let thumb_rel = if poster_url.is_empty() {
        String::new()
    } else {
        match download_poster(client, &poster_url, &manager.paths.videos_thumbs, &unique).await {
            Ok(rel) => rel,
            Err(err) => {
                tracing::warn!("video poster download failed for {}: {}", poster_url, err);
                String::new()
            }
        }
    };

    let thumb_cleanup = if thumb_rel.is_empty() {
        None
    } else {
        Some(manager.paths.root.join(&thumb_rel))
    };
    let new = build_new_image(
        result,
        DownloadedMediaParts {
            filename,
            path_rel: original_rel,
            thumb_rel,
            width: result.width.unwrap_or(0),
            height: result.height.unwrap_or(0),
            file_size,
            preview_only,
        },
    );
    let manager_for_db = manager.clone();
    let added = tokio::task::spawn_blocking(move || manager_for_db.add_image(&new))
        .await
        .map_err(|e| AppError::other(e.to_string()))?;
    match added {
        Ok(saved) => {
            tracing::info!(
                "download saved: {} {} {}x{} id={}",
                saved.source,
                saved.kind,
                saved.width,
                saved.height,
                saved.id
            );
            Ok(Some(saved))
        }
        Err(e) => {
            if let Some(p) = &original_full {
                let _ = tokio::fs::remove_file(p).await;
            }
            if let Some(p) = &thumb_cleanup {
                let _ = tokio::fs::remove_file(p).await;
            }
            tracing::warn!(
                "add_image failed for {} ({e}); cleaned up partial files",
                result.url
            );
            Ok(None)
        }
    }
}

async fn download_poster(
    client: &Client,
    url: &str,
    thumbs_dir: &std::path::Path,
    unique: &str,
) -> Result<String> {
    let (bytes, _content_type) =
        fetch_capped(client, url, MAX_PHOTO_BYTES, Duration::from_secs(30)).await?;
    // Re-encode to a 300px JPEG thumbnail for consistency
    let bytes_for_decode = bytes;
    let (_, _, thumb_jpeg) = tokio::task::spawn_blocking(move || -> Result<(u32, u32, Vec<u8>)> {
        let img = image::load_from_memory(&bytes_for_decode)?;
        let thumb = img.thumbnail(THUMB_SIZE, THUMB_SIZE).to_rgb8();
        let (tw, th) = thumb.dimensions();
        let mut out = Vec::with_capacity(64 * 1024);
        JpegEncoder::new_with_quality(&mut out, THUMB_QUALITY).write_image(
            thumb.as_raw(),
            tw,
            th,
            ExtendedColorType::Rgb8,
        )?;
        Ok((tw, th, out))
    })
    .await
    .map_err(|e| AppError::other(e.to_string()))??;

    let filename = format!("vthumb_{}.jpg", &unique[..12]);
    let full = thumbs_dir.join(&filename);
    write_atomic(&full, &thumb_jpeg).await?;
    Ok(format!("videos/thumbs/{}", filename))
}

pub async fn download_many(
    client: Client,
    manager: Arc<ImageManager>,
    results: Vec<SearchResult>,
    preview_only: bool,
    concurrency: usize,
) -> Vec<Image> {
    let _active = DownloadGuard::new();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(
        concurrency.clamp(1, MAX_DOWNLOAD_CONCURRENCY),
    ));
    let mut handles = Vec::with_capacity(results.len());

    for result in results {
        let permit = semaphore.clone();
        let client = client.clone();
        let manager = manager.clone();
        handles.push(tokio::spawn(async move {
            let _p = permit.acquire_owned().await.ok()?;
            match download_one(&client, &manager, &result, preview_only).await {
                Ok(Some(img)) => Some(img),
                Ok(None) => None,
                Err(err) => {
                    tracing::warn!("download failed for {}: {}", result.url, err);
                    None
                }
            }
        }));
    }

    let mut saved = Vec::new();
    for h in handles {
        if let Ok(Some(img)) = h.await {
            saved.push(img);
        }
    }
    saved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_extension_never_falls_back_to_an_image_type() {
        assert_eq!(video_ext("https://cdn.test/a.mp4?x=1", ""), ".mp4");
        assert_eq!(video_ext("https://cdn.test/a.WEBM", ""), ".webm");
        assert_eq!(video_ext("https://cdn.test/a.mov#t=1", ""), ".mov");
        assert_eq!(
            video_ext("https://cdn.test/download/123", "video/webm"),
            ".webm"
        );
        assert_eq!(
            video_ext("https://cdn.test/download/123", "video/quicktime"),
            ".mov"
        );
        // No hint at all: mp4, not .jpg.
        assert_eq!(video_ext("https://cdn.test/download/123", ""), ".mp4");
        assert_eq!(
            video_ext("https://cdn.test/download/123", "application/octet-stream"),
            ".mp4"
        );
    }

    #[test]
    fn part_path_appends_to_the_full_name() {
        assert_eq!(
            part_path(Path::new("videos/originals/clip_1x1_ab.mp4")),
            PathBuf::from("videos/originals/clip_1x1_ab.mp4.part")
        );
    }

    #[test]
    fn download_guard_tracks_active_work() {
        let before = active_downloads();
        let a = DownloadGuard::new();
        let b = DownloadGuard::new();
        assert!(active_downloads() >= before + 2);
        drop(a);
        drop(b);
    }
}
