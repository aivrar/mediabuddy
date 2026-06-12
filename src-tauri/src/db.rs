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
        "#,
    )?;
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
