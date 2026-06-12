//! Persistent search topics. A topic is one user-curated exploration of a
//! (query, filters, kind) tuple. Across sessions, repeating that tuple
//! looks up the same topic and resumes pagination from where it left off,
//! so a user can build up a large collection by repeatedly hitting "Get
//! more" without re-fetching pages they've already evaluated.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use reqwest::Client;
use tokio::sync::RwLock;

use crate::config::Settings;
use crate::error::{AppError, Result};
use crate::image_manager::ImageManager;
use crate::quota::QuotaTracker;
use crate::search::{self, SearchFilters, SearchResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Topic {
    pub id: String,
    pub name: Option<String>,
    pub query: String,
    pub filters: SearchFilters,
    pub kind: String, // "photo" | "video" | "both"
    pub enabled_sources: Vec<String>,
    pub created_at: String,
    pub last_fetched_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicCursor {
    pub source: String,
    pub media_kind: String,
    pub next_page: u32,
    pub total_seen: u64,
    pub last_status: String, // "pending" | "ok" | "empty" | "error"
    pub last_fetched_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicSummary {
    pub id: String,
    pub name: Option<String>,
    pub query: String,
    pub kind: String,
    pub enabled_sources: Vec<String>,
    pub created_at: String,
    pub last_fetched_at: Option<String>,
    pub seen_count: u64,
    pub saved_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicStatus {
    pub topic: Topic,
    pub cursors: Vec<TopicCursor>,
    pub seen_count: u64,
    pub saved_count: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum CursorStatus {
    Ok,
    Empty,
    Error,
}

impl CursorStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CursorStatus::Ok => "ok",
            CursorStatus::Empty => "empty",
            CursorStatus::Error => "error",
        }
    }
}

pub struct TopicStore {
    conn: Arc<Mutex<Connection>>,
}

impl TopicStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Look up an existing topic for `(query, filters, kind)` or create one.
    /// Filter values are sorted/canonicalized before hashing so cosmetically
    /// different but semantically equal filter sets collide on the same topic.
    ///
    /// If the topic already exists and the caller passes a different (or
    /// expanded) `enabled_sources` set, we merge — keeping any sources the
    /// topic already had AND adding any new ones, plus seeding cursors for
    /// the new (source, media_kind) pairs. We never silently drop a source
    /// that was previously active, since that would orphan its cursor.
    pub fn find_or_create(
        &self,
        query: &str,
        filters: &SearchFilters,
        kind: &str,
        enabled_sources: &[String],
    ) -> Result<Topic> {
        if query.trim().is_empty() {
            return Err(AppError::other(
                "topic query is empty; type something in the search box first",
            ));
        }
        let filters_json = canonical_filters_json(filters);
        let conn = self.conn.lock().unwrap();
        let existing_row = conn
            .query_row(
                "SELECT id, name, query, filters_json, kind, enabled_sources, created_at, last_fetched_at
                 FROM topics WHERE query = ?1 AND filters_json = ?2 AND kind = ?3",
                params![query, &filters_json, kind],
                topic_row_mapper,
            )
            .ok();

        if let Some(existing) = row_to_topic(existing_row)? {
            let mut merged: Vec<String> = existing.enabled_sources.clone();
            let mut newly_added: Vec<String> = Vec::new();
            for s in enabled_sources {
                if !merged.iter().any(|x| x.eq_ignore_ascii_case(s.as_str())) {
                    merged.push(s.clone());
                    newly_added.push(s.clone());
                }
            }
            if !newly_added.is_empty() {
                let sources_json = serde_json::to_string(&merged)?;
                conn.execute(
                    "UPDATE topics SET enabled_sources = ?2 WHERE id = ?1",
                    params![existing.id, &sources_json],
                )?;
                for src in &newly_added {
                    for media in cursor_kinds_for(src, kind) {
                        conn.execute(
                            "INSERT OR IGNORE INTO topic_cursors
                             (topic_id, source, media_kind, next_page, last_status)
                             VALUES (?1, ?2, ?3, 1, 'pending')",
                            params![existing.id, src, media],
                        )?;
                    }
                }
                return Ok(Topic {
                    enabled_sources: merged,
                    ..existing
                });
            }
            return Ok(existing);
        }

        let id = Uuid::new_v4().to_string();
        let sources_json = serde_json::to_string(enabled_sources)?;
        conn.execute(
            "INSERT INTO topics (id, query, filters_json, kind, enabled_sources)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, query, &filters_json, kind, &sources_json],
        )?;

        for src in enabled_sources {
            for media in cursor_kinds_for(src, kind) {
                conn.execute(
                    "INSERT OR IGNORE INTO topic_cursors
                     (topic_id, source, media_kind, next_page, last_status)
                     VALUES (?1, ?2, ?3, 1, 'pending')",
                    params![id, src, media],
                )?;
            }
        }

        let row = conn
            .query_row(
                "SELECT id, name, query, filters_json, kind, enabled_sources, created_at, last_fetched_at
                 FROM topics WHERE id = ?1",
                params![id],
                topic_row_mapper,
            )
            .map_err(AppError::from)?;
        Ok(row_to_topic(Some(row))?.unwrap())
    }

    pub fn get(&self, topic_id: &str) -> Result<Option<Topic>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, name, query, filters_json, kind, enabled_sources, created_at, last_fetched_at
                 FROM topics WHERE id = ?1",
                params![topic_id],
                topic_row_mapper,
            )
            .ok();
        row_to_topic(row)
    }

    pub fn cursors_for(&self, topic_id: &str) -> Result<Vec<TopicCursor>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT source, media_kind, next_page, total_seen, last_status, last_fetched_at
             FROM topic_cursors WHERE topic_id = ?1
             ORDER BY source, media_kind",
        )?;
        let rows = stmt.query_map(params![topic_id], |r| {
            Ok(TopicCursor {
                source: r.get(0)?,
                media_kind: r.get(1)?,
                next_page: r.get::<_, i64>(2)? as u32,
                total_seen: r.get::<_, i64>(3)? as u64,
                last_status: r.get(4)?,
                last_fetched_at: r.get(5).ok(),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn status(&self, topic_id: &str) -> Result<Option<TopicStatus>> {
        let topic = match self.get(topic_id)? {
            Some(t) => t,
            None => return Ok(None),
        };
        let cursors = self.cursors_for(topic_id)?;
        let conn = self.conn.lock().unwrap();
        let seen_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM topic_seen WHERE topic_id = ?1",
                params![topic_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        // saved_count: how many of the items we've seen for this topic
        // ended up downloaded. Joining topic_seen.source_id against
        // images.source_id is the correct identity match (URLs vary by
        // tier per source, but source_id is the provider's stable id).
        let saved_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM topic_seen ts
                 INNER JOIN images i
                   ON lower(i.source) = lower(ts.source) AND i.source_id = ts.source_id
                 WHERE ts.topic_id = ?1",
                params![topic_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(Some(TopicStatus {
            topic,
            cursors,
            seen_count: seen_count as u64,
            saved_count: saved_count as u64,
        }))
    }

    pub fn list_summaries(&self) -> Result<Vec<TopicSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                t.id,
                t.name,
                t.query,
                t.kind,
                t.enabled_sources,
                t.created_at,
                t.last_fetched_at,
                COALESCE((SELECT COUNT(*) FROM topic_seen ts WHERE ts.topic_id = t.id), 0) AS seen_count,
                COALESCE((
                    SELECT COUNT(*) FROM topic_seen ts2
                    INNER JOIN images i ON lower(i.source) = lower(ts2.source) AND i.source_id = ts2.source_id
                    WHERE ts2.topic_id = t.id
                ), 0) AS saved_count
            FROM topics t
            ORDER BY COALESCE(t.last_fetched_at, t.created_at) DESC
            "#,
        )?;
        let rows = stmt.query_map([], |r| {
            let sources_json: String = r.get(4)?;
            let enabled_sources: Vec<String> =
                serde_json::from_str(&sources_json).unwrap_or_default();
            Ok(TopicSummary {
                id: r.get(0)?,
                name: r.get(1).ok(),
                query: r.get(2)?,
                kind: r.get(3)?,
                enabled_sources,
                created_at: r.get(5)?,
                last_fetched_at: r.get(6).ok(),
                seen_count: r.get::<_, i64>(7)? as u64,
                saved_count: r.get::<_, i64>(8)? as u64,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn delete(&self, topic_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM topic_seen WHERE topic_id = ?1",
            params![topic_id],
        )?;
        conn.execute(
            "DELETE FROM topic_cursors WHERE topic_id = ?1",
            params![topic_id],
        )?;
        conn.execute("DELETE FROM topics WHERE id = ?1", params![topic_id])?;
        Ok(())
    }

    /// Wipe pagination + seen state. Subsequent searches start at page 1
    /// again, useful when the user wants to re-discover items.
    pub fn reset(&self, topic_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM topic_seen WHERE topic_id = ?1",
            params![topic_id],
        )?;
        conn.execute(
            "UPDATE topic_cursors
             SET next_page = 1, total_seen = 0, last_status = 'pending', last_fetched_at = NULL
             WHERE topic_id = ?1",
            params![topic_id],
        )?;
        Ok(())
    }

    pub fn rename(&self, topic_id: &str, name: Option<String>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE topics SET name = ?2 WHERE id = ?1",
            params![topic_id, name],
        )?;
        Ok(())
    }

    /// `images.id`s in the library that this topic has touched. Used to
    /// scope the Library grid to one topic's downloads.
    pub fn topic_image_ids(&self, topic_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT i.id FROM images i
             INNER JOIN topic_seen ts
               ON lower(ts.source) = lower(i.source) AND ts.source_id = i.source_id
             WHERE ts.topic_id = ?1
             ORDER BY i.downloaded_at DESC",
        )?;
        let rows = stmt.query_map(params![topic_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Returns the seen `source_id`s for a `(topic, source)` pair so the
    /// caller can drop already-evaluated results.
    pub fn seen_ids(&self, topic_id: &str, source: &str) -> Result<BTreeSet<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT source_id FROM topic_seen WHERE topic_id = ?1 AND source = ?2")?;
        let rows = stmt.query_map(params![topic_id, source], |r| r.get::<_, String>(0))?;
        let mut out = BTreeSet::new();
        for r in rows {
            out.insert(r?);
        }
        Ok(out)
    }

    /// Insert a batch of `(source, source_id)` entries into the seen set.
    /// Existing rows are silently ignored.
    pub fn record_seen(
        &self,
        topic_id: &str,
        source: &str,
        source_ids: &[String],
    ) -> Result<usize> {
        if source_ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO topic_seen (topic_id, source, source_id)
                 VALUES (?1, ?2, ?3)",
            )?;
            for sid in source_ids {
                if sid.is_empty() {
                    continue;
                }
                inserted += stmt.execute(params![topic_id, source, sid])?;
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// Advance a cursor after a successful fetch. `pages_fetched` should
    /// normally be 1 for a single round; the cursor moves by that amount.
    pub fn advance_cursor(
        &self,
        topic_id: &str,
        source: &str,
        media_kind: &str,
        pages_fetched: u32,
        total_seen_delta: u64,
        status: CursorStatus,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE topic_cursors
             SET next_page = next_page + ?4,
                 total_seen = total_seen + ?5,
                 last_status = ?6,
                 last_fetched_at = datetime('now')
             WHERE topic_id = ?1 AND source = ?2 AND media_kind = ?3",
            params![
                topic_id,
                source,
                media_kind,
                pages_fetched as i64,
                total_seen_delta as i64,
                status.as_str(),
            ],
        )?;
        // Also bump the topic's last_fetched_at.
        conn.execute(
            "UPDATE topics SET last_fetched_at = datetime('now') WHERE id = ?1",
            params![topic_id],
        )?;
        Ok(())
    }
}

// ---------- helpers ----------

fn cursor_kinds_for(source: &str, kind: &str) -> Vec<&'static str> {
    let want_photo = matches!(kind, "photo" | "both");
    let want_video = matches!(kind, "video" | "both");
    let mut out = Vec::new();
    if want_photo {
        out.push("photo");
    }
    if want_video && !source.eq_ignore_ascii_case("unsplash") {
        // Unsplash has no video API; don't seed a cursor that can never advance.
        out.push("video");
    }
    out
}

fn canonical_filters_json(filters: &SearchFilters) -> String {
    // serde_json by default emits keys in struct-declaration order, which
    // is stable. Trimming/normalizing string values keeps cosmetically
    // identical filter sets aligned. We do not currently reorder keys
    // because the struct is fixed.
    let mut copy = filters.clone();
    copy.orientation = copy.orientation.map(|s| s.trim().to_lowercase());
    copy.color = copy.color.map(|s| s.trim().to_lowercase());
    copy.category = copy.category.map(|s| s.trim().to_lowercase());
    copy.order = copy.order.map(|s| s.trim().to_lowercase());
    copy.image_type = copy.image_type.map(|s| s.trim().to_lowercase());
    copy.video_type = copy.video_type.map(|s| s.trim().to_lowercase());
    copy.size = copy.size.map(|s| s.trim().to_lowercase());
    // count_per_source is *not* a topic identity — the user can ask for
    // different chunk sizes against the same topic.
    copy.count_per_source = None;
    serde_json::to_string(&copy).unwrap_or_else(|_| "{}".into())
}

type TopicRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
);

fn topic_row_mapper(r: &rusqlite::Row<'_>) -> rusqlite::Result<TopicRow> {
    Ok((
        r.get(0)?,
        r.get(1).ok(),
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7).ok(),
    ))
}

fn row_to_topic(row: Option<TopicRow>) -> Result<Option<Topic>> {
    let r = match row {
        Some(r) => r,
        None => return Ok(None),
    };
    let filters: SearchFilters = serde_json::from_str(&r.3).unwrap_or_default();
    let enabled_sources: Vec<String> = serde_json::from_str(&r.5).unwrap_or_default();
    Ok(Some(Topic {
        id: r.0,
        name: r.1,
        query: r.2,
        filters,
        kind: r.4,
        enabled_sources,
        created_at: r.6,
        last_fetched_at: r.7,
    }))
}

// ============================================================================
// Engine — cursor-driven "Get more" pagination.
// ============================================================================

/// One result of `topic_get_more`. `results` are the new items not yet in
/// the library and not yet in `topic_seen`. Per-source progress info lets
/// the UI show which cursors advanced and which exhausted.
#[derive(Debug, Clone, Serialize)]
pub struct TopicGetMoreResult {
    pub topic_id: String,
    pub results: Vec<SearchResult>,
    pub progress: Vec<TopicProgress>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicProgress {
    pub source: String,
    pub media_kind: String,
    pub page_fetched: u32,
    pub raw_count: u32,  // results returned by the provider
    pub kept_count: u32, // after de-dup against seen + saved
    pub status: String,  // "ok" | "empty" | "error"
}

/// Fetch one round of new results for the topic. Each cursor with status
/// 'ok' or 'pending' fires a single page request; results are filtered
/// against `topic_seen` and the global URL-saved set, then `topic_seen` is
/// updated and the cursor advanced. Cursors that returned zero raw results
/// flip to `status='empty'` so future rounds skip them.
pub async fn topic_get_more(
    store: &TopicStore,
    topic_id: &str,
    client: &Client,
    settings: Arc<RwLock<Settings>>,
    manager: Arc<ImageManager>,
    tracker: Option<Arc<QuotaTracker>>,
    count_override: Option<u32>,
) -> Result<TopicGetMoreResult> {
    let topic = store
        .get(topic_id)?
        .ok_or_else(|| AppError::other(format!("topic not found: {topic_id}")))?;

    let snap = settings.read().await.clone();
    let cursors = store.cursors_for(topic_id)?;

    let unsplash_detail_threshold: u32 = snap.unsplash_detail_threshold;
    let count_per_source = count_override
        .or(topic.filters.count_per_source)
        .unwrap_or(80)
        .max(1);

    let mut all_results: Vec<SearchResult> = Vec::new();
    let mut progress: Vec<TopicProgress> = Vec::new();

    for cursor in cursors {
        if cursor.last_status == "empty" {
            // Source has run out of new pages — skip it on subsequent
            // rounds. The user can `topic_reset` to start over.
            continue;
        }
        // Skip sources the user has disabled on the topic.
        if !topic
            .enabled_sources
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&cursor.source))
        {
            continue;
        }

        let (per_page, key) = match cursor.source.as_str() {
            "pixabay" => (count_per_source.clamp(3, 200), snap.pixabay_key.clone()),
            "pexels" => (count_per_source.clamp(3, 80), snap.pexels_key.clone()),
            "unsplash" => (count_per_source.clamp(3, 30), snap.unsplash_key.clone()),
            _ => continue,
        };

        if key.is_empty() {
            // Don't pretend to fetch from a source the user hasn't keyed up.
            continue;
        }

        let res_raw = match (cursor.source.as_str(), cursor.media_kind.as_str()) {
            ("pixabay", "photo") => {
                search::search_pixabay(
                    client,
                    &key,
                    &topic.query,
                    &topic.filters,
                    cursor.next_page,
                    per_page,
                    tracker.as_ref(),
                )
                .await
            }
            ("pixabay", "video") => {
                search::search_pixabay_videos(
                    client,
                    &key,
                    &topic.query,
                    &topic.filters,
                    cursor.next_page,
                    per_page,
                    tracker.as_ref(),
                )
                .await
            }
            ("pexels", "photo") => {
                search::search_pexels(
                    client,
                    &key,
                    &topic.query,
                    &topic.filters,
                    cursor.next_page,
                    per_page,
                    tracker.as_ref(),
                )
                .await
            }
            ("pexels", "video") => {
                search::search_pexels_videos(
                    client,
                    &key,
                    &topic.query,
                    &topic.filters,
                    cursor.next_page,
                    per_page,
                    tracker.as_ref(),
                )
                .await
            }
            ("unsplash", "photo") => {
                let fetch_details = per_page <= unsplash_detail_threshold;
                search::search_unsplash(
                    client,
                    &key,
                    &topic.query,
                    &topic.filters,
                    tracker.as_ref(),
                    search::UnsplashSearchOptions {
                        page: cursor.next_page,
                        per_page,
                        fetch_details,
                    },
                )
                .await
            }
            _ => continue,
        };

        let raw = match res_raw {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "topic {topic_id} {} {} fetch failed: {e}",
                    cursor.source,
                    cursor.media_kind
                );
                store.advance_cursor(
                    topic_id,
                    &cursor.source,
                    &cursor.media_kind,
                    0, // don't advance page on error — retry next round
                    0,
                    CursorStatus::Error,
                )?;
                progress.push(TopicProgress {
                    source: cursor.source.clone(),
                    media_kind: cursor.media_kind.clone(),
                    page_fetched: cursor.next_page,
                    raw_count: 0,
                    kept_count: 0,
                    status: "error".into(),
                });
                continue;
            }
        };

        let raw_count = raw.len() as u32;

        // Drop items already in the library — by URL (any tier) or by
        // (source, source_id) so re-listings at a different URL tier don't
        // sneak through.
        let raw: Vec<SearchResult> = raw
            .into_iter()
            .filter(|r| {
                !r.url.is_empty()
                    && !manager.is_url_saved(&r.url)
                    && !manager.is_source_id_saved(&r.source, &r.source_id)
            })
            .collect();

        // Drop items we've already seen for this topic.
        let seen = store.seen_ids(topic_id, &cursor.source)?;
        let kept: Vec<SearchResult> = raw
            .into_iter()
            .filter(|r| {
                let sid = r.source_id.trim();
                !sid.is_empty() && !seen.contains(sid)
            })
            .collect();

        // Record everything we just touched as seen.
        let new_ids: Vec<String> = kept.iter().map(|r| r.source_id.clone()).collect();
        let _ = store.record_seen(topic_id, &cursor.source, &new_ids);

        let kept_count = kept.len() as u32;
        let status = if raw_count == 0 {
            CursorStatus::Empty
        } else {
            CursorStatus::Ok
        };
        let pages_to_advance = if raw_count == 0 { 0 } else { 1 };

        store.advance_cursor(
            topic_id,
            &cursor.source,
            &cursor.media_kind,
            pages_to_advance,
            kept_count as u64,
            status,
        )?;

        progress.push(TopicProgress {
            source: cursor.source.clone(),
            media_kind: cursor.media_kind.clone(),
            page_fetched: cursor.next_page,
            raw_count,
            kept_count,
            status: status.as_str().into(),
        });

        all_results.extend(kept);
    }

    Ok(TopicGetMoreResult {
        topic_id: topic_id.to_string(),
        results: all_results,
        progress,
    })
}
