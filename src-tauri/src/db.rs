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
