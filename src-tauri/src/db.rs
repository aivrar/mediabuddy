use std::path::Path;

use rusqlite::Connection;

use crate::error::Result;

pub fn open(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    init_schema(&conn)?;
    migrate(&conn)?;
    migrate_data(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS images (
            id TEXT PRIMARY KEY,
            filename TEXT,
            path TEXT,
            thumb_path TEXT,
            url TEXT NOT NULL UNIQUE,
            source TEXT NOT NULL,
            query TEXT NOT NULL,
            width INTEGER,
            height INTEGER,
            alt TEXT,
            tags TEXT,
            preview_only INTEGER DEFAULT 0,
            downloaded_at TEXT DEFAULT (datetime('now')),
            vision_processed INTEGER DEFAULT 0
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_url ON images(url);

        CREATE TABLE IF NOT EXISTS blocked_urls (
            url TEXT PRIMARY KEY,
            source TEXT,
            deleted_at TEXT DEFAULT (datetime('now'))
        );

        -- Persistent search topic. A topic is one user-curated exploration
        -- of a query+filters combination. Reusing the same query/filters
        -- looks up the existing topic so pagination cursors carry across
        -- sessions.
        CREATE TABLE IF NOT EXISTS topics (
            id TEXT PRIMARY KEY,
            name TEXT,
            query TEXT NOT NULL,
            filters_json TEXT NOT NULL DEFAULT '{}',
            kind TEXT NOT NULL DEFAULT 'photo',
            enabled_sources TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_fetched_at TEXT,
            UNIQUE(query, filters_json, kind)
        );

        -- Pagination cursor for one (topic, source, media_kind). The next
        -- "Get more" round fetches `next_page` from each cursor that is
        -- still 'ok'. Cursors flip to 'empty' once a fetch returns zero
        -- new items so we stop calling them.
        CREATE TABLE IF NOT EXISTS topic_cursors (
            topic_id TEXT NOT NULL,
            source TEXT NOT NULL,
            media_kind TEXT NOT NULL,
            next_page INTEGER NOT NULL DEFAULT 1,
            total_seen INTEGER NOT NULL DEFAULT 0,
            last_status TEXT NOT NULL DEFAULT 'pending',
            last_fetched_at TEXT,
            PRIMARY KEY (topic_id, source, media_kind)
        );

        -- Every (source, source_id) we've shown to the user under this
        -- topic, whether or not they downloaded it. Lets a follow-up
        -- search skip results we already evaluated.
        CREATE TABLE IF NOT EXISTS topic_seen (
            topic_id TEXT NOT NULL,
            source TEXT NOT NULL,
            source_id TEXT NOT NULL,
            seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (topic_id, source, source_id)
        );

        CREATE INDEX IF NOT EXISTS idx_topic_seen_topic ON topic_seen(topic_id);
        CREATE INDEX IF NOT EXISTS idx_topic_cursors_topic ON topic_cursors(topic_id);

        -- Results a topic has fetched that the user has not yet downloaded
        -- or dismissed. Persisted so closing the app or running another
        -- search doesn't lose candidates the cursor has already moved past.
        -- `source_key` is the same identity used in `topic_seen.source_id`
        -- (videos carry a `video:` prefix).
        CREATE TABLE IF NOT EXISTS topic_pending (
            topic_id TEXT NOT NULL,
            source TEXT NOT NULL,
            source_key TEXT NOT NULL,
            result_json TEXT NOT NULL,
            fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (topic_id, source, source_key)
        );
        "#,
    )?;
    Ok(())
}

/// Prefix that marks a video identity in `topic_seen.source_id` and
/// `topic_pending.source_key`. Providers number photos and videos in separate
/// sequences, so the bare id alone is ambiguous.
pub const VIDEO_KEY_PREFIX: &str = "video:";

/// One-time data migrations, tracked with `PRAGMA user_version`.
fn migrate_data(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        // v1: give video rows in `topic_seen` their own key space. Video-only
        // topics are unambiguous. For mixed topics only rows that match a
        // saved video (and no saved image with the same id) are rewritten;
        // anything else stays as an image key.
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE topic_seen SET source_id = 'video:' || source_id
             WHERE source_id NOT LIKE 'video:%'
               AND topic_id IN (SELECT id FROM topics WHERE lower(kind) IN ('video', 'videos'))",
            [],
        )?;
        tx.execute(
            "UPDATE topic_seen SET source_id = 'video:' || source_id
             WHERE source_id NOT LIKE 'video:%'
               AND topic_id IN (SELECT id FROM topics WHERE lower(kind) NOT IN ('video', 'videos', 'photo', 'photos'))
               AND EXISTS (
                   SELECT 1 FROM images i
                   WHERE lower(i.source) = lower(topic_seen.source)
                     AND i.source_id = topic_seen.source_id
                     AND lower(COALESCE(i.kind, 'photo')) = 'video')
               AND NOT EXISTS (
                   SELECT 1 FROM images i
                   WHERE lower(i.source) = lower(topic_seen.source)
                     AND i.source_id = topic_seen.source_id
                     AND lower(COALESCE(i.kind, 'photo')) != 'video')",
            [],
        )?;
        tx.commit()?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    // Each ALTER TABLE is wrapped — SQLite errors if column already exists.
    let migrations: &[&str] = &[
        "ALTER TABLE images ADD COLUMN source_id TEXT DEFAULT ''",
        "ALTER TABLE images ADD COLUMN kind TEXT DEFAULT 'photo'",
        "ALTER TABLE images ADD COLUMN source_page_url TEXT DEFAULT ''",
        "ALTER TABLE images ADD COLUMN urls TEXT DEFAULT '{}'",
        "ALTER TABLE images ADD COLUMN duration_secs INTEGER",
        "ALTER TABLE images ADD COLUMN file_size INTEGER",
        "ALTER TABLE images ADD COLUMN color TEXT",
        "ALTER TABLE images ADD COLUMN blur_hash TEXT",
        "ALTER TABLE images ADD COLUMN author_name TEXT DEFAULT ''",
        "ALTER TABLE images ADD COLUMN author_url TEXT DEFAULT ''",
        "ALTER TABLE images ADD COLUMN author_avatar TEXT DEFAULT ''",
        "ALTER TABLE images ADD COLUMN views INTEGER",
        "ALTER TABLE images ADD COLUMN downloads INTEGER",
        "ALTER TABLE images ADD COLUMN likes INTEGER",
        "ALTER TABLE images ADD COLUMN comments INTEGER",
        "ALTER TABLE images ADD COLUMN ai_generated INTEGER",
        "ALTER TABLE images ADD COLUMN created_at_provider TEXT",
        "ALTER TABLE images ADD COLUMN source_data TEXT DEFAULT '{}'",
        // Who wrote the caption in `alt`: 'provider' | 'ai' | 'manual' | ''
        // ('' = no caption, or an older row whose origin was never recorded).
        "ALTER TABLE images ADD COLUMN caption_source TEXT DEFAULT ''",
        // Identity of a deleted item, so "don't re-show what I deleted"
        // survives a restart even when the provider re-lists it at a new URL.
        "ALTER TABLE blocked_urls ADD COLUMN source_id TEXT DEFAULT ''",
        "ALTER TABLE blocked_urls ADD COLUMN kind TEXT DEFAULT ''",
        // Items consumed from the provider so far. Lets the page size change
        // between rounds without skipping results (NULL on older rows).
        "ALTER TABLE topic_cursors ADD COLUMN item_offset INTEGER",
    ];
    for sql in migrations {
        // Ignore "duplicate column" errors so migrations are idempotent.
        if let Err(err) = conn.execute(sql, []) {
            let msg = err.to_string();
            if !msg.contains("duplicate column name") {
                tracing::warn!("migration step skipped ({}): {}", sql, err);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Schema as shipped in 0.1.0, before caption_source / blocked identity /
    /// item_offset / topic_pending existed.
    const LEGACY_SCHEMA: &str = r#"
        CREATE TABLE images (
            id TEXT PRIMARY KEY, filename TEXT, path TEXT, thumb_path TEXT,
            url TEXT NOT NULL UNIQUE, source TEXT NOT NULL, query TEXT NOT NULL,
            width INTEGER, height INTEGER, alt TEXT, tags TEXT,
            preview_only INTEGER DEFAULT 0,
            downloaded_at TEXT DEFAULT (datetime('now')),
            vision_processed INTEGER DEFAULT 0,
            source_id TEXT DEFAULT '', kind TEXT DEFAULT 'photo'
        );
        CREATE TABLE blocked_urls (
            url TEXT PRIMARY KEY, source TEXT,
            deleted_at TEXT DEFAULT (datetime('now'))
        );
        CREATE TABLE topics (
            id TEXT PRIMARY KEY, name TEXT, query TEXT NOT NULL,
            filters_json TEXT NOT NULL DEFAULT '{}',
            kind TEXT NOT NULL DEFAULT 'photo',
            enabled_sources TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_fetched_at TEXT, UNIQUE(query, filters_json, kind)
        );
        CREATE TABLE topic_cursors (
            topic_id TEXT NOT NULL, source TEXT NOT NULL, media_kind TEXT NOT NULL,
            next_page INTEGER NOT NULL DEFAULT 1,
            total_seen INTEGER NOT NULL DEFAULT 0,
            last_status TEXT NOT NULL DEFAULT 'pending', last_fetched_at TEXT,
            PRIMARY KEY (topic_id, source, media_kind)
        );
        CREATE TABLE topic_seen (
            topic_id TEXT NOT NULL, source TEXT NOT NULL, source_id TEXT NOT NULL,
            seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (topic_id, source, source_id)
        );
    "#;

    fn seen(conn: &Connection, topic: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT source_id FROM topic_seen WHERE topic_id = ?1 ORDER BY source_id")
            .unwrap();
        let rows = stmt.query_map([topic], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    #[test]
    fn an_existing_library_upgrades_in_place() {
        let dir = std::env::temp_dir().join(format!(
            "mediabuddy-dbtest-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("images.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(LEGACY_SCHEMA).unwrap();
            conn.execute_batch(
                r#"
                INSERT INTO images (id, url, source, query, alt, source_id, kind) VALUES
                    ('p1', 'https://x.test/p1.jpg', 'Pixabay', 'q', 'old caption', '10', 'photo'),
                    ('v1', 'https://x.test/v1.mp4', 'Pixabay', 'q', '', '20', 'video'),
                    ('p2', 'https://x.test/p2.jpg', 'Pixabay', 'q', '', '30', 'photo'),
                    ('v2', 'https://x.test/v2.mp4', 'Pixabay', 'q', '', '30', 'video');
                INSERT INTO blocked_urls (url, source) VALUES ('https://x.test/gone.jpg', 'Pixabay');
                INSERT INTO topics (id, query, kind) VALUES
                    ('tp', 'photos', 'photo'), ('tv', 'videos', 'video'), ('tb', 'mixed', 'both');
                INSERT INTO topic_cursors (topic_id, source, media_kind, next_page)
                    VALUES ('tp', 'pixabay', 'photo', 4);
                INSERT INTO topic_seen (topic_id, source, source_id) VALUES
                    ('tp', 'pixabay', '10'),
                    ('tv', 'pixabay', '20'),
                    ('tb', 'pixabay', '10'),
                    ('tb', 'pixabay', '20'),
                    ('tb', 'pixabay', '30'),
                    ('tb', 'pixabay', '99');
                "#,
            )
            .unwrap();
        }

        // Opening twice proves the migrations are safe to re-run.
        drop(open(&path).unwrap());
        let conn = open(&path).unwrap();

        // Existing rows keep their data; origin of old captions is unknown.
        let (alt, source): (String, String) = conn
            .query_row(
                "SELECT alt, caption_source FROM images WHERE id = 'p1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((alt.as_str(), source.as_str()), ("old caption", ""));

        // The old cursor keeps its page and has no offset yet.
        let (page, offset): (i64, Option<i64>) = conn
            .query_row(
                "SELECT next_page, item_offset FROM topic_cursors WHERE topic_id = 'tp'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((page, offset), (4, None));

        // Photo topic untouched; video topic fully re-keyed.
        assert_eq!(seen(&conn, "tp"), vec!["10"]);
        assert_eq!(seen(&conn, "tv"), vec!["video:20"]);
        // Mixed topic: 20 matches only a saved video, so it is re-keyed.
        // 10 (photo), 30 (a photo and a video share it) and 99 (not in the
        // library) stay as image keys.
        assert_eq!(seen(&conn, "tb"), vec!["10", "30", "99", "video:20"]);

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
        // New table and columns exist.
        conn.execute(
            "INSERT INTO topic_pending (topic_id, source, source_key, result_json) \
             VALUES ('tp', 'pixabay', '1', '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE blocked_urls SET source_id = '5', kind = 'photo'",
            [],
        )
        .unwrap();
        drop(conn);
        let _ = std::fs::remove_dir_all(dir);
    }
}
