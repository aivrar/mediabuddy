use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::db;
use crate::error::{AppError, Result};
use crate::paths::AppPaths;
use crate::types::{
    DeleteResult, Image, NewImage, CAPTION_SOURCE_AI, CAPTION_SOURCE_MANUAL,
    CAPTION_SOURCE_PROVIDER,
};
use crate::vision::CaptionPolicy;

/// Identity of a provider item: `(source lowercased, is_video, source_id)`.
/// Providers number photos and videos in separate sequences, so the media
/// class is part of the key — a saved photo must not hide an unrelated video
/// that happens to share its numeric id.
type SourceKey = (String, bool, String);

fn source_key(source: &str, kind: &str, source_id: &str) -> SourceKey {
    (
        source.trim().to_ascii_lowercase(),
        kind.trim().eq_ignore_ascii_case("video"),
        source_id.trim().to_string(),
    )
}

pub struct ImageManager {
    conn: Arc<Mutex<Connection>>,
    /// URLs that must not be downloaded again: everything in the library
    /// plus everything in `blocked_urls`.
    existing_urls: Arc<RwLock<HashSet<String>>>,
    /// The `blocked_urls` subset of `existing_urls` (items the user deleted).
    blocked_urls: Arc<RwLock<HashSet<String>>>,
    /// Provider identities already in the library or deleted from it. Used
    /// to dedup search results that re-list the same item at a different
    /// URL tier (Pixabay's webformat vs large, etc).
    existing_source_ids: Arc<RwLock<HashSet<SourceKey>>>,
    pub paths: AppPaths,
}

/// Result of writing one Florence-2 analysis back to the library.
#[derive(Debug, Clone)]
pub struct VisionApplyOutcome {
    pub caption_written: bool,
    pub tags_added: Vec<String>,
    pub image: Image,
}

/// Merge detected-object labels into an existing tag list, case-insensitively.
/// Returns `(merged, added)`; `added` is empty when nothing new was found.
pub fn merge_label_tags(existing: &[String], labels: &[String]) -> (Vec<String>, Vec<String>) {
    let mut merged = Vec::with_capacity(existing.len() + labels.len());
    let mut seen = HashSet::new();
    for tag in existing {
        let trimmed = tag.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
            merged.push(trimmed.to_string());
        }
    }
    let mut added = Vec::new();
    for label in labels {
        let trimmed = label.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
            merged.push(trimmed.to_string());
            added.push(trimmed.to_string());
        }
    }
    (merged, added)
}

impl ImageManager {
    /// Hands out a clone of the shared connection handle for modules that
    /// store related data in the same SQLite file (e.g. topics).
    pub fn conn_handle(&self) -> Arc<Mutex<Connection>> {
        self.conn.clone()
    }

    /// Poison-tolerant lock: a panic on another thread while it held the
    /// connection must not take every later library call down with it.
    fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// True if we've ever ingested this provider item — including items the
    /// user later deleted. Deletion blocks the URL via `blocked_urls`, but a
    /// provider can re-list the same identity at a different URL tier; the
    /// deleted identity is stored alongside the blocked URL so "don't
    /// re-show me what I've already evaluated" holds across URL-tier churn
    /// and across restarts.
    pub fn is_source_id_saved(&self, source: &str, kind: &str, source_id: &str) -> bool {
        if source_id.trim().is_empty() {
            return false;
        }
        let key = source_key(source, kind, source_id);
        self.existing_source_ids
            .read()
            .map(|set| set.contains(&key))
            .unwrap_or(false)
    }
}

impl ImageManager {
    pub fn new(paths: AppPaths) -> Result<Self> {
        let conn = db::open(&paths.db)?;
        let blocked = Self::load_blocked_urls(&conn)?;
        let existing_urls = Self::load_existing_urls(&conn, &blocked)?;
        let existing_source_ids = Self::load_existing_source_ids(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            existing_urls: Arc::new(RwLock::new(existing_urls)),
            blocked_urls: Arc::new(RwLock::new(blocked)),
            existing_source_ids: Arc::new(RwLock::new(existing_source_ids)),
            paths,
        })
    }

    fn load_blocked_urls(conn: &Connection) -> Result<HashSet<String>> {
        let mut urls = HashSet::new();
        let mut stmt = conn.prepare("SELECT url FROM blocked_urls")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            urls.insert(row?);
        }
        Ok(urls)
    }

    fn load_existing_urls(conn: &Connection, blocked: &HashSet<String>) -> Result<HashSet<String>> {
        let mut urls = blocked.clone();
        let mut stmt = conn.prepare("SELECT url FROM images")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            urls.insert(row?);
        }
        Ok(urls)
    }

    fn load_existing_source_ids(conn: &Connection) -> Result<HashSet<SourceKey>> {
        let mut set = HashSet::new();
        for sql in [
            "SELECT source, COALESCE(kind, ''), source_id FROM images WHERE source_id != ''",
            "SELECT COALESCE(source, ''), COALESCE(kind, ''), source_id FROM blocked_urls \
             WHERE source_id IS NOT NULL AND source_id != ''",
        ] {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for r in rows {
                let (source, kind, source_id) = r?;
                set.insert(source_key(&source, &kind, &source_id));
            }
        }
        Ok(set)
    }

    pub fn is_url_saved(&self, url: &str) -> bool {
        self.existing_urls
            .read()
            .map(|set| set.contains(url))
            .unwrap_or(false)
    }

    /// True if this URL is refused because the user deleted the item (as
    /// opposed to it being in the library right now).
    pub fn is_url_blocked(&self, url: &str) -> bool {
        self.blocked_urls
            .read()
            .map(|set| set.contains(url))
            .unwrap_or(false)
    }

    pub fn blocked_count(&self) -> usize {
        self.blocked_urls.read().map(|set| set.len()).unwrap_or(0)
    }

    /// Forget every deleted item so it can be found and downloaded again.
    /// Returns how many blocked URLs were cleared.
    pub fn clear_blocked(&self) -> Result<usize> {
        let conn = self.lock_conn();
        let cleared = conn.execute("DELETE FROM blocked_urls", [])?;
        let blocked = HashSet::new();
        let urls = Self::load_existing_urls(&conn, &blocked)?;
        let ids = Self::load_existing_source_ids(&conn)?;
        // Swap the caches while still holding the connection so a concurrent
        // delete can't interleave between the DELETE and the rebuild.
        if let Ok(mut set) = self.blocked_urls.write() {
            *set = blocked;
        }
        if let Ok(mut set) = self.existing_urls.write() {
            *set = urls;
        }
        if let Ok(mut set) = self.existing_source_ids.write() {
            *set = ids;
        }
        Ok(cleared)
    }

    pub fn add_image(&self, new: &NewImage) -> Result<Image> {
        let id = Uuid::new_v4().to_string();
        let tags_json = serde_json::to_string(&new.tags)?;
        let urls_json = serde_json::to_string(&new.urls)?;
        let source_data_json = serde_json::to_string(&new.source_data)?;
        // Whatever caption arrives with a download is the provider's.
        let caption_source = if new.alt.trim().is_empty() {
            ""
        } else {
            CAPTION_SOURCE_PROVIDER
        };
        let downloaded_at = {
            let conn = self.lock_conn();
            let inserted = conn.execute(
                r#"
                INSERT OR IGNORE INTO images
                (id, filename, path, thumb_path, url, source, query,
                 width, height, alt, tags, preview_only,
                 source_id, kind, source_page_url, urls,
                 duration_secs, file_size, color, blur_hash,
                 author_name, author_url, author_avatar,
                 views, downloads, likes, comments, ai_generated,
                 created_at_provider, source_data, caption_source)
                VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                 ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31)
                "#,
                params![
                    id,
                    new.filename,
                    new.path,
                    new.thumb_path,
                    new.url,
                    new.source,
                    new.query,
                    new.width,
                    new.height,
                    new.alt,
                    tags_json,
                    new.preview_only as i32,
                    new.source_id,
                    new.kind,
                    new.source_page_url,
                    urls_json,
                    new.duration_secs,
                    new.file_size,
                    new.color,
                    new.blur_hash,
                    new.author_name,
                    new.author_url,
                    new.author_avatar,
                    new.views,
                    new.downloads,
                    new.likes,
                    new.comments,
                    new.ai_generated.map(|b| b as i32),
                    new.created_at_provider,
                    source_data_json,
                    caption_source,
                ],
            )?;
            if inserted == 0 {
                // A row with this URL already exists (UNIQUE(url)) and the
                // INSERT was ignored. Return a clean duplicate error so the
                // caller can roll back the files it just wrote, instead of
                // failing on a SELECT-by-new-id that finds nothing.
                return Err(AppError::other("duplicate: url already in library"));
            }
            conn.query_row(
                "SELECT downloaded_at FROM images WHERE id = ?1",
                params![id],
                |r| r.get::<_, String>(0),
            )?
        };

        if let Ok(mut set) = self.existing_urls.write() {
            set.insert(new.url.clone());
        }
        if !new.source_id.trim().is_empty() {
            if let Ok(mut set) = self.existing_source_ids.write() {
                set.insert(source_key(&new.source, &new.kind, &new.source_id));
            }
        }

        Ok(Image {
            id,
            source: new.source.clone(),
            source_id: new.source_id.clone(),
            kind: new.kind.clone(),
            source_page_url: new.source_page_url.clone(),
            filename: new.filename.clone(),
            path: new.path.clone(),
            thumb_path: new.thumb_path.clone(),
            url: new.url.clone(),
            urls: new.urls.clone(),
            width: new.width,
            height: new.height,
            duration_secs: new.duration_secs,
            file_size: new.file_size,
            query: new.query.clone(),
            alt: new.alt.clone(),
            caption_source: caption_source.to_string(),
            tags: new.tags.clone(),
            color: new.color.clone(),
            blur_hash: new.blur_hash.clone(),
            author_name: new.author_name.clone(),
            author_url: new.author_url.clone(),
            author_avatar: new.author_avatar.clone(),
            views: new.views,
            downloads: new.downloads,
            likes: new.likes,
            comments: new.comments,
            preview_only: new.preview_only,
            vision_processed: false,
            ai_generated: new.ai_generated,
            created_at_provider: new.created_at_provider.clone(),
            downloaded_at,
            source_data: new.source_data.clone(),
        })
    }

    pub fn get_all_images(&self) -> Result<Vec<Image>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(SELECT_ALL_SQL)?;
        let rows = stmt.query_map([], image_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_image_by_id(&self, id: &str) -> Result<Option<Image>> {
        let conn = self.lock_conn();
        select_image(&conn, id)
    }

    /// Apply a person's edit to the caption and/or tags (Inspector, bulk tag
    /// edit, REST update). A caption that actually changes is recorded as
    /// manual so later AI runs can leave it alone; re-saving the same text
    /// keeps whatever origin it already had. Returns the updated row, or
    /// `None` if the item no longer exists.
    pub fn update_image_metadata(
        &self,
        id: &str,
        alt: Option<String>,
        tags: Option<Vec<String>>,
    ) -> Result<Option<Image>> {
        let conn = self.lock_conn();
        if alt.is_none() && tags.is_none() {
            return select_image(&conn, id);
        }
        let tags_json = match &tags {
            Some(t) => Some(serde_json::to_string(t)?),
            None => None,
        };
        // SQLite evaluates every SET expression against the row as it was
        // before the UPDATE, so `caption_source` sees the old `alt` here.
        // `?2 IS NULL` means "leave this column alone".
        let changed = conn.execute(
            r#"
            UPDATE images SET
                caption_source = CASE
                    WHEN ?2 IS NULL OR ?2 = COALESCE(alt, '') THEN caption_source
                    WHEN trim(?2) = '' THEN ''
                    ELSE ?4
                END,
                alt = COALESCE(?2, alt),
                tags = COALESCE(?3, tags)
            WHERE id = ?1
            "#,
            params![id, alt, tags_json, CAPTION_SOURCE_MANUAL],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        select_image(&conn, id)
    }

    /// Write one Florence-2 result back to the library.
    ///
    /// Inference takes seconds per image, and a person (or a REST client) can
    /// edit the same row meanwhile. So the row is re-read here, under the
    /// connection lock, and both decisions — whether the caption may be
    /// replaced, and which tags are new — are made against what is in the
    /// database *now*, not against a copy taken before inference started.
    ///
    /// Returns `None` if the item was deleted while it was being analyzed.
    pub fn apply_vision_result(
        &self,
        id: &str,
        caption: &str,
        labels: &[String],
        policy: &CaptionPolicy,
    ) -> Result<Option<VisionApplyOutcome>> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let current = tx
            .query_row(
                "SELECT COALESCE(alt, ''), COALESCE(tags, '[]'), COALESCE(caption_source, '') \
                 FROM images WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((current_alt, current_tags_json, current_source)) = current else {
            return Ok(None);
        };

        let caption = caption.trim();
        let caption_written = policy.should_write(&current_alt, &current_source, caption);
        let current_tags: Vec<String> =
            serde_json::from_str(&current_tags_json).unwrap_or_default();
        let (merged_tags, tags_added) = merge_label_tags(&current_tags, labels);

        if caption_written {
            tx.execute(
                "UPDATE images SET alt = ?2, caption_source = ?3 WHERE id = ?1",
                params![id, caption, CAPTION_SOURCE_AI],
            )?;
        }
        if !tags_added.is_empty() {
            tx.execute(
                "UPDATE images SET tags = ?2 WHERE id = ?1",
                params![id, serde_json::to_string(&merged_tags)?],
            )?;
        }
        tx.execute(
            "UPDATE images SET vision_processed = 1 WHERE id = ?1",
            params![id],
        )?;
        let image = select_image(&tx, id)?;
        tx.commit()?;

        Ok(image.map(|image| VisionApplyOutcome {
            caption_written,
            tags_added,
            image,
        }))
    }

    /// Add tags to an item without replacing the ones it has. The merge runs
    /// against the stored list inside a transaction, so tags written since
    /// the caller last loaded the item (an edit elsewhere, an analysis run)
    /// are kept. Returns `None` if the item no longer exists.
    pub fn append_tags(&self, id: &str, tags: &[String]) -> Result<Option<Image>> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let current: Option<String> = tx
            .query_row(
                "SELECT COALESCE(tags, '[]') FROM images WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(current) = current else {
            return Ok(None);
        };
        let current: Vec<String> = serde_json::from_str(&current).unwrap_or_default();
        let (mut merged, added) = merge_label_tags(&current, tags);
        if !added.is_empty() {
            merged.sort();
            tx.execute(
                "UPDATE images SET tags = ?2 WHERE id = ?1",
                params![id, serde_json::to_string(&merged)?],
            )?;
        }
        let image = select_image(&tx, id)?;
        tx.commit()?;
        Ok(image)
    }

    pub fn delete_images(&self, ids: &[String]) -> Result<DeleteResult> {
        if ids.is_empty() {
            return Ok(DeleteResult {
                deleted: 0,
                failed: 0,
            });
        }
        let mut deleted: usize = 0;
        let mut failed: usize = 0;
        // Relative paths to remove AFTER the DB transaction commits. Removing
        // files before commit risks a rollback leaving the DB pointing at
        // already-deleted files; committing first means a failed removal at
        // worst orphans a file while the DB stays consistent.
        let mut files_to_remove: Vec<String> = Vec::new();
        let mut newly_blocked: Vec<String> = Vec::new();

        {
            let mut conn = self.lock_conn();
            let tx = conn.transaction()?;

            for id in ids {
                let row = tx.query_row(
                    "SELECT url, path, thumb_path, source, source_id, kind FROM images WHERE id = ?1",
                    params![id],
                    |r| {
                        let url: String = r.get(0)?;
                        let path: Option<String> = r.get(1)?;
                        let thumb: Option<String> = r.get(2)?;
                        let source: Option<String> = r.get(3)?;
                        let source_id: Option<String> = r.get(4)?;
                        let kind: Option<String> = r.get(5)?;
                        Ok((url, path, thumb, source, source_id, kind))
                    },
                );
                let (url, path, thumb, source, source_id, kind) = match row {
                    Ok(t) => t,
                    Err(_) => {
                        failed += 1;
                        continue;
                    }
                };
                tx.execute("DELETE FROM images WHERE id = ?1", params![id])?;
                // Keep the provider identity with the blocked URL. The
                // in-memory `existing_source_ids` entry is left in place and
                // this row is what restores it after a restart.
                tx.execute(
                    "INSERT OR IGNORE INTO blocked_urls (url, source, source_id, kind) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        url,
                        source.unwrap_or_default(),
                        source_id.unwrap_or_default(),
                        kind.unwrap_or_default()
                    ],
                )?;
                newly_blocked.push(url);
                if let Some(p) = path {
                    if !p.is_empty() {
                        files_to_remove.push(p);
                    }
                }
                if let Some(p) = thumb {
                    if !p.is_empty() {
                        files_to_remove.push(p);
                    }
                }
                deleted += 1;
            }
            tx.commit()?;
        }

        if let Ok(mut set) = self.blocked_urls.write() {
            set.extend(newly_blocked);
        }

        // DB committed and consistent — now remove files best-effort.
        for rel in files_to_remove {
            let _ = std::fs::remove_file(self.paths.root.join(&rel));
        }

        Ok(DeleteResult { deleted, failed })
    }
}

const IMAGE_COLUMNS: &str = "id, filename, path, thumb_path, url, source, query, width, height, alt, tags, preview_only, downloaded_at, vision_processed, source_id, kind, source_page_url, urls, duration_secs, file_size, color, blur_hash, author_name, author_url, author_avatar, views, downloads, likes, comments, ai_generated, created_at_provider, source_data, caption_source";

const SELECT_ALL_SQL: &str = "SELECT id, filename, path, thumb_path, url, source, query, width, height, alt, tags, preview_only, downloaded_at, vision_processed, source_id, kind, source_page_url, urls, duration_secs, file_size, color, blur_hash, author_name, author_url, author_avatar, views, downloads, likes, comments, ai_generated, created_at_provider, source_data, caption_source FROM images ORDER BY downloaded_at DESC";

fn select_image(conn: &Connection, id: &str) -> Result<Option<Image>> {
    let sql = format!("SELECT {IMAGE_COLUMNS} FROM images WHERE id = ?1");
    Ok(conn
        .query_row(&sql, params![id], image_from_row)
        .optional()?)
}

fn image_from_row(r: &Row<'_>) -> rusqlite::Result<Image> {
    let tags_json: String = r.get::<_, Option<String>>(10)?.unwrap_or_default();
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    let urls_json: String = r
        .get::<_, Option<String>>(17)?
        .unwrap_or_else(|| "{}".to_string());
    let urls: Value = serde_json::from_str(&urls_json).unwrap_or_else(|_| json!({}));
    let source_data_json: String = r
        .get::<_, Option<String>>(31)?
        .unwrap_or_else(|| "{}".to_string());
    let source_data: Value = serde_json::from_str(&source_data_json).unwrap_or(Value::Null);
    let kind: String = r
        .get::<_, Option<String>>(15)?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "photo".to_string());

    let ai_generated_int: Option<i64> = r.get(29)?;
    let ai_generated = ai_generated_int.map(|i| i != 0);

    Ok(Image {
        id: r.get(0)?,
        filename: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        path: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        thumb_path: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        url: r.get(4)?,
        source: r.get(5)?,
        query: r.get(6)?,
        width: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
        height: r.get::<_, Option<i64>>(8)?.unwrap_or(0),
        alt: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
        caption_source: r.get::<_, Option<String>>(32)?.unwrap_or_default(),
        tags,
        preview_only: r.get::<_, i64>(11)? != 0,
        downloaded_at: r.get(12)?,
        vision_processed: r.get::<_, i64>(13)? != 0,
        source_id: r.get::<_, Option<String>>(14)?.unwrap_or_default(),
        kind,
        source_page_url: r.get::<_, Option<String>>(16)?.unwrap_or_default(),
        urls,
        duration_secs: r.get(18)?,
        file_size: r.get(19)?,
        color: r.get(20)?,
        blur_hash: r.get(21)?,
        author_name: r.get::<_, Option<String>>(22)?.unwrap_or_default(),
        author_url: r.get::<_, Option<String>>(23)?.unwrap_or_default(),
        author_avatar: r.get::<_, Option<String>>(24)?.unwrap_or_default(),
        views: r.get(25)?,
        downloads: r.get(26)?,
        likes: r.get(27)?,
        comments: r.get(28)?,
        ai_generated,
        created_at_provider: r.get(30)?,
        source_data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::CaptionWriteMode;

    fn temp_manager(name: &str) -> (ImageManager, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mediabuddy-test-{name}-{}",
            Uuid::new_v4().simple()
        ));
        let paths = AppPaths::at(&root).expect("temp app paths");
        (ImageManager::new(paths).expect("image manager"), root)
    }

    fn new_image(url: &str, alt: &str, kind: &str, source_id: &str) -> NewImage {
        NewImage {
            source: "Pixabay".into(),
            source_id: source_id.into(),
            kind: kind.into(),
            source_page_url: String::new(),
            filename: "f.jpg".into(),
            path: "images/originals/f.jpg".into(),
            thumb_path: String::new(),
            url: url.into(),
            urls: json!({}),
            width: 10,
            height: 10,
            duration_secs: None,
            file_size: None,
            query: "q".into(),
            alt: alt.into(),
            tags: vec!["sky".into()],
            color: None,
            blur_hash: None,
            author_name: String::new(),
            author_url: String::new(),
            author_avatar: String::new(),
            views: None,
            downloads: None,
            likes: None,
            comments: None,
            preview_only: false,
            ai_generated: None,
            created_at_provider: None,
            source_data: json!({}),
        }
    }

    fn policy(mode: CaptionWriteMode) -> CaptionPolicy {
        CaptionPolicy {
            mode,
            min_chars: 80,
            protect_manual: true,
        }
    }

    #[test]
    fn caption_source_tracks_who_wrote_it() {
        let (mgr, root) = temp_manager("source");
        let with_alt = mgr
            .add_image(&new_image(
                "https://x.test/a.jpg",
                "A provider line",
                "photo",
                "1",
            ))
            .unwrap();
        let without = mgr
            .add_image(&new_image("https://x.test/b.jpg", "", "photo", "2"))
            .unwrap();
        assert_eq!(with_alt.caption_source, CAPTION_SOURCE_PROVIDER);
        assert_eq!(without.caption_source, "");

        // Saving tags only, or re-saving the same caption, keeps the origin.
        let same = mgr
            .update_image_metadata(
                &with_alt.id,
                Some("A provider line".into()),
                Some(vec!["sky".into(), "blue".into()]),
            )
            .unwrap()
            .unwrap();
        assert_eq!(same.caption_source, CAPTION_SOURCE_PROVIDER);
        assert_eq!(same.tags, vec!["sky", "blue"]);

        // A real edit is manual; clearing the caption clears the origin.
        let edited = mgr
            .update_image_metadata(&with_alt.id, Some("My own words".into()), None)
            .unwrap()
            .unwrap();
        assert_eq!(edited.caption_source, CAPTION_SOURCE_MANUAL);
        assert_eq!(edited.tags, vec!["sky", "blue"]);
        let cleared = mgr
            .update_image_metadata(&with_alt.id, Some(String::new()), None)
            .unwrap()
            .unwrap();
        assert_eq!(cleared.caption_source, "");

        assert!(mgr
            .update_image_metadata("missing", Some("x".into()), None)
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn appending_tags_merges_into_the_stored_list() {
        let (mgr, root) = temp_manager("append");
        let img = mgr
            .add_image(&new_image(
                "https://x.test/a.jpg",
                "A caption",
                "photo",
                "1",
            ))
            .unwrap();

        // The stored tags changed after the caller loaded the item.
        mgr.update_image_metadata(
            &img.id,
            None,
            Some(vec!["tree".into(), "added elsewhere".into()]),
        )
        .unwrap();

        let saved = mgr
            .append_tags(&img.id, &["new".into(), "Tree".into(), " ".into()])
            .unwrap()
            .unwrap();
        assert_eq!(saved.tags, vec!["added elsewhere", "new", "tree"]);
        assert_eq!(saved.alt, "A caption");
        assert_eq!(saved.caption_source, CAPTION_SOURCE_PROVIDER);

        assert!(mgr.append_tags("missing", &["x".into()]).unwrap().is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn vision_write_uses_the_row_as_it_is_now() {
        let (mgr, root) = temp_manager("vision");
        let img = mgr
            .add_image(&new_image("https://x.test/a.jpg", "", "photo", "1"))
            .unwrap();

        // Simulates an edit landing while inference was running: the caption
        // is now manual and a tag was removed and another added.
        mgr.update_image_metadata(
            &img.id,
            Some("Typed during the job".into()),
            Some(vec!["mine".into()]),
        )
        .unwrap();

        let out = mgr
            .apply_vision_result(
                &img.id,
                "A generated caption",
                &["Tree".into(), "mine".into()],
                &policy(CaptionWriteMode::Overwrite),
            )
            .unwrap()
            .unwrap();
        assert!(!out.caption_written, "manual caption must be kept");
        assert_eq!(out.image.alt, "Typed during the job");
        assert_eq!(out.image.caption_source, CAPTION_SOURCE_MANUAL);
        assert_eq!(out.tags_added, vec!["Tree"]);
        assert_eq!(out.image.tags, vec!["mine", "Tree"]);
        assert!(out.image.vision_processed);

        // With protection off, overwrite replaces it and records the origin.
        let mut open = policy(CaptionWriteMode::Overwrite);
        open.protect_manual = false;
        let out = mgr
            .apply_vision_result(&img.id, "A generated caption", &[], &open)
            .unwrap()
            .unwrap();
        assert!(out.caption_written);
        assert_eq!(out.image.caption_source, CAPTION_SOURCE_AI);

        assert!(mgr
            .apply_vision_result("missing", "x", &[], &open)
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn deleted_identity_survives_restart_and_can_be_cleared() {
        let (mgr, root) = temp_manager("blocked");
        let photo = mgr
            .add_image(&new_image("https://x.test/a.jpg", "", "photo", "77"))
            .unwrap();
        // Same numeric id, different media class: must not collide.
        assert!(mgr.is_source_id_saved("pixabay", "photo", "77"));
        assert!(!mgr.is_source_id_saved("Pixabay", "video", "77"));

        mgr.delete_images(&[photo.id]).unwrap();
        assert!(mgr.is_url_blocked("https://x.test/a.jpg"));
        assert_eq!(mgr.blocked_count(), 1);

        let paths = mgr.paths.clone();
        drop(mgr);
        let reopened = ImageManager::new(paths).unwrap();
        assert!(reopened.is_url_saved("https://x.test/a.jpg"));
        assert!(reopened.is_source_id_saved("Pixabay", "photo", "77"));

        assert_eq!(reopened.clear_blocked().unwrap(), 1);
        assert!(!reopened.is_url_saved("https://x.test/a.jpg"));
        assert!(!reopened.is_source_id_saved("Pixabay", "photo", "77"));
        assert_eq!(reopened.blocked_count(), 0);
        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
    }
}
