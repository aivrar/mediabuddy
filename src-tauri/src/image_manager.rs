use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};

use rusqlite::{params, Connection, Row};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::db;
use crate::error::{AppError, Result};
use crate::paths::AppPaths;
use crate::types::{DeleteResult, Image, NewImage};

pub struct ImageManager {
    conn: Arc<Mutex<Connection>>,
    existing_urls: Arc<RwLock<HashSet<String>>>,
    /// `(source, source_id)` pairs of items already in the library. Used
    /// to dedup search results that re-list the same item at a different
    /// URL tier (Pixabay's webformat vs large, etc).
    existing_source_ids: Arc<RwLock<HashSet<(String, String)>>>,
    pub paths: AppPaths,
}

impl ImageManager {
    /// Hands out a clone of the shared connection handle for modules that
    /// store related data in the same SQLite file (e.g. topics).
    pub fn conn_handle(&self) -> Arc<Mutex<Connection>> {
        self.conn.clone()
    }

    /// True if we've ever ingested this `(source, source_id)` — including
    /// items the user later deleted. Deletion blocks the URL via
    /// `blocked_urls`, but a provider can re-list the same identity at a
    /// different URL tier; keeping deleted ids in the in-memory set means
    /// "don't re-show me what I've already evaluated" actually holds across
    /// URL-tier churn.
    pub fn is_source_id_saved(&self, source: &str, source_id: &str) -> bool {
        if source_id.is_empty() {
            return false;
        }
        let key = (source.to_string(), source_id.to_string());
        self.existing_source_ids
            .read()
            .map(|set| set.contains(&key))
            .unwrap_or(false)
    }
}

impl ImageManager {
    pub fn new(paths: AppPaths) -> Result<Self> {
        let conn = db::open(&paths.db)?;
        let existing_urls = Self::load_existing_urls(&conn)?;
        let existing_source_ids = Self::load_existing_source_ids(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            existing_urls: Arc::new(RwLock::new(existing_urls)),
            existing_source_ids: Arc::new(RwLock::new(existing_source_ids)),
            paths,
        })
    }

    fn load_existing_urls(conn: &Connection) -> Result<HashSet<String>> {
        let mut urls = HashSet::new();
        let mut stmt = conn.prepare("SELECT url FROM images")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            urls.insert(row?);
        }
        let mut stmt = conn.prepare("SELECT url FROM blocked_urls")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            urls.insert(row?);
        }
        Ok(urls)
    }

    fn load_existing_source_ids(conn: &Connection) -> Result<HashSet<(String, String)>> {
        let mut set = HashSet::new();
        let mut stmt =
            conn.prepare("SELECT source, source_id FROM images WHERE source_id != ''")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for r in rows {
            set.insert(r?);
        }
        Ok(set)
    }

    pub fn is_url_saved(&self, url: &str) -> bool {
        self.existing_urls
            .read()
            .map(|set| set.contains(url))
            .unwrap_or(false)
    }

    pub fn add_image(&self, new: &NewImage) -> Result<Image> {
        let id = Uuid::new_v4().to_string();
        let tags_json = serde_json::to_string(&new.tags)?;
        let urls_json = serde_json::to_string(&new.urls)?;
        let source_data_json = serde_json::to_string(&new.source_data)?;
        let downloaded_at = {
            let conn = self.conn.lock().unwrap();
            let inserted = conn.execute(
                r#"
                INSERT OR IGNORE INTO images
                (id, filename, path, thumb_path, url, source, query,
                 width, height, alt, tags, preview_only,
                 source_id, kind, source_page_url, urls,
                 duration_secs, file_size, color, blur_hash,
                 author_name, author_url, author_avatar,
                 views, downloads, likes, comments, ai_generated,
                 created_at_provider, source_data)
                VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                 ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)
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
        if !new.source_id.is_empty() {
            if let Ok(mut set) = self.existing_source_ids.write() {
                set.insert((new.source.clone(), new.source_id.clone()));
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(SELECT_ALL_SQL)?;
        let rows = stmt.query_map([], image_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_image_by_id(&self, id: &str) -> Result<Option<Image>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(SELECT_ONE_SQL)?;
        let mut rows = stmt.query_map(params![id], image_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn update_image_metadata(
        &self,
        id: &str,
        alt: Option<String>,
        tags: Option<Vec<String>>,
        vision_processed: Option<bool>,
    ) -> Result<bool> {
        if alt.is_none() && tags.is_none() && vision_processed.is_none() {
            return Ok(false);
        }
        let mut sets: Vec<&str> = Vec::new();
        let mut vals: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(a) = alt {
            sets.push("alt = ?");
            vals.push(rusqlite::types::Value::Text(a));
        }
        if let Some(t) = tags {
            sets.push("tags = ?");
            vals.push(rusqlite::types::Value::Text(serde_json::to_string(&t)?));
        }
        if let Some(vp) = vision_processed {
            sets.push("vision_processed = ?");
            vals.push(rusqlite::types::Value::Integer(if vp { 1 } else { 0 }));
        }
        let sql = format!("UPDATE images SET {} WHERE id = ?", sets.join(", "));
        vals.push(rusqlite::types::Value::Text(id.to_string()));

        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(&sql, rusqlite::params_from_iter(vals.iter()))?;
        Ok(changed > 0)
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

        {
            let mut conn = self.conn.lock().unwrap();
            let tx = conn.transaction()?;

            for id in ids {
                let row = tx.query_row(
                    "SELECT url, path, thumb_path, source FROM images WHERE id = ?1",
                    params![id],
                    |r| {
                        let url: String = r.get(0)?;
                        let path: Option<String> = r.get(1)?;
                        let thumb: Option<String> = r.get(2)?;
                        let source: Option<String> = r.get(3)?;
                        Ok((url, path, thumb, source))
                    },
                );
                let (url, path, thumb, source) = match row {
                    Ok(t) => t,
                    Err(_) => {
                        failed += 1;
                        continue;
                    }
                };
                tx.execute("DELETE FROM images WHERE id = ?1", params![id])?;
                tx.execute(
                    "INSERT OR IGNORE INTO blocked_urls (url, source) VALUES (?1, ?2)",
                    params![url, source.unwrap_or_default()],
                )?;
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
                // We intentionally keep the `(source, source_id)` in
                // `existing_source_ids` so a future search at a different URL
                // tier still skips the item the user already evaluated.
                deleted += 1;
            }
            tx.commit()?;
        }

        // DB committed and consistent — now remove files best-effort.
        for rel in files_to_remove {
            let _ = std::fs::remove_file(self.paths.root.join(&rel));
        }

        Ok(DeleteResult { deleted, failed })
    }
}

const SELECT_ALL_SQL: &str = "SELECT id, filename, path, thumb_path, url, source, query, width, height, alt, tags, preview_only, downloaded_at, vision_processed, source_id, kind, source_page_url, urls, duration_secs, file_size, color, blur_hash, author_name, author_url, author_avatar, views, downloads, likes, comments, ai_generated, created_at_provider, source_data FROM images ORDER BY downloaded_at DESC";

const SELECT_ONE_SQL: &str = "SELECT id, filename, path, thumb_path, url, source, query, width, height, alt, tags, preview_only, downloaded_at, vision_processed, source_id, kind, source_page_url, urls, duration_secs, file_size, color, blur_hash, author_name, author_url, author_avatar, views, downloads, likes, comments, ai_generated, created_at_provider, source_data FROM images WHERE id = ?1";

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
