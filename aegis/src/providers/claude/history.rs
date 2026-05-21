//! Conversation history store. Persists every voice turn (user transcript
//! + Claude reply) so we can search past conversations later.
//!
//! Storage: SQLite database at `~/.config/aegis/history.db`. One row per
//! turn. Future tiers will add an `embedding` BLOB column and an FTS5
//! virtual table for hybrid semantic + keyword search.
//!
//! Design notes for the tier-3 plan live in `docs/memory-architecture.md`.
//!
//! Current scope (step 4): open the db, create the table + FTS5 index,
//! record turns synchronously, read by id, and run keyword search via
//! FTS5 with BM25 ranking. Still no async writer, no embeddings.

use rusqlite::Connection;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single conversation turn as stored in the database. `id` is assigned
/// by SQLite on insert and is what callers use to fetch a row back.
/// `ts` is unix epoch seconds.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TurnRecord {
    pub id: i64,
    pub ts: i64,
    pub user_text: String,
    pub claude_text: String,
}

/// Current unix epoch in seconds. Convenience for callers building a
/// `record()` call without pulling in chrono or `SystemTime` boilerplate.
#[allow(dead_code)]
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Handle to the SQLite-backed conversation log. Cheap to construct: the
/// real work is opening the connection inside `open` / `open_default`.
// dead_code stays until the orchestrator hookup lands; later steps add
// the async writer and the `search_history` tool surface.
#[allow(dead_code)]
pub struct HistoryStore {
    /// Path to the underlying .db file. Held so callers can re-open
    /// short-lived read connections without re-resolving the config dir.
    pub path: PathBuf,
}

#[allow(dead_code)]
impl HistoryStore {
    /// Open the store at the default location (`~/.config/aegis/history.db`
    /// on Linux, the OS equivalent elsewhere). Creates the parent
    /// directory if missing. Missing db file is fine; we create the
    /// schema on first open.
    pub fn open_default() -> Result<Self, Box<dyn std::error::Error>> {
        let mut path = dirs::config_dir().ok_or("could not locate config dir")?;
        path.push("aegis");
        std::fs::create_dir_all(&path)?;
        path.push("history.db");
        Self::open(path)
    }

    /// Open the store at an explicit path. Used by tests with a tmp file
    /// and by callers that want to override the default location.
    /// Idempotent: opening an existing db with the same schema is a
    /// no-op beyond setting the pragmas.
    pub fn open(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&path)?;

        // WAL mode: concurrent reads while a write is in progress, and
        // crash-safe writes without fsync-on-every-commit overhead.
        // synchronous=NORMAL is the right tradeoff for a single-user
        // voice assistant. Durable across crashes, not across sudden
        // power loss in the last few hundred ms.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        // Base table plus the FTS5 keyword-search machinery. The
        // virtual table indexes the two text columns; the three
        // triggers keep it in sync with the source table on every
        // insert / delete / update so our Rust code never has to.
        // External content table (`content='turns'`) avoids duplicating
        // the text on disk: FTS5 stores tokens only, joins back to
        // turns for the actual values. Tokenizer choice: porter for
        // stemming ("run"/"running"/"ran" collapse), unicode61 for
        // proper handling of non-ASCII, remove_diacritics so accented
        // and unaccented forms match.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS turns (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                ts          INTEGER NOT NULL,
                user_text   TEXT NOT NULL,
                claude_text TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS turns_fts USING fts5(
                user_text, claude_text,
                content='turns',
                content_rowid='id',
                tokenize='porter unicode61 remove_diacritics 2'
            );

            CREATE TRIGGER IF NOT EXISTS turns_ai AFTER INSERT ON turns BEGIN
                INSERT INTO turns_fts(rowid, user_text, claude_text)
                VALUES (new.id, new.user_text, new.claude_text);
            END;

            CREATE TRIGGER IF NOT EXISTS turns_ad AFTER DELETE ON turns BEGIN
                INSERT INTO turns_fts(turns_fts, rowid, user_text, claude_text)
                VALUES ('delete', old.id, old.user_text, old.claude_text);
            END;

            CREATE TRIGGER IF NOT EXISTS turns_au AFTER UPDATE ON turns BEGIN
                INSERT INTO turns_fts(turns_fts, rowid, user_text, claude_text)
                VALUES ('delete', old.id, old.user_text, old.claude_text);
                INSERT INTO turns_fts(rowid, user_text, claude_text)
                VALUES (new.id, new.user_text, new.claude_text);
            END;",
        )?;

        // One-time backfill: if `turns` has rows that aren't in the
        // FTS index, rebuild the index from the source table. Handles
        // the case where an existing pre-FTS5 database (from step 2)
        // is opened by the new code. No-op on fresh databases and on
        // already-synced ones.
        let turns_count: i64 = conn.query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))?;
        let fts_count: i64 = conn.query_row("SELECT COUNT(*) FROM turns_fts", [], |r| r.get(0))?;
        if turns_count > fts_count {
            conn.execute_batch("INSERT INTO turns_fts(turns_fts) VALUES('rebuild');")?;
            eprintln!(
                "[history] FTS backfill: rebuilt index from {} existing turns",
                turns_count
            );
        }

        eprintln!("[history] opened db at {}", path.display());

        Ok(Self { path })
    }

    /// Insert one turn into the database and return its assigned id.
    ///
    /// Synchronous and blocking. A later step moves writes onto a
    /// background task so the voice loop doesn't pay disk-write
    /// latency on the hot path.
    pub fn record(
        &self,
        ts: i64,
        user_text: &str,
        claude_text: &str,
    ) -> Result<i64, Box<dyn std::error::Error>> {
        let conn = Connection::open(&self.path)?;
        conn.execute(
            "INSERT INTO turns (ts, user_text, claude_text) VALUES (?, ?, ?)",
            rusqlite::params![ts, user_text, claude_text],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Fetch a single turn by id. Returns `Ok(None)` when no row matches
    /// the id (not an error; just empty). Returns `Err` on real I/O or
    /// schema problems.
    pub fn get_by_id(&self, id: i64) -> Result<Option<TurnRecord>, Box<dyn std::error::Error>> {
        let conn = Connection::open(&self.path)?;
        let result = conn.query_row(
            "SELECT id, ts, user_text, claude_text FROM turns WHERE id = ?",
            rusqlite::params![id],
            |row| {
                Ok(TurnRecord {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    user_text: row.get(2)?,
                    claude_text: row.get(3)?,
                })
            },
        );
        match result {
            Ok(rec) => Ok(Some(rec)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Keyword search over recorded turns. Returns up to `limit` rows
    /// ordered by BM25 relevance (best match first).
    ///
    /// `query` is passed straight to FTS5 and supports its full query
    /// syntax: bare words (`tokyo flight`), phrases (`"good morning"`),
    /// boolean ops (`flight AND NOT cancelled`), prefix matches (`tok*`).
    ///
    /// No matches returns an empty Vec, not an error. `Err` is reserved
    /// for malformed FTS queries and I/O failures.
    pub fn search_keyword(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<TurnRecord>, Box<dyn std::error::Error>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT t.id, t.ts, t.user_text, t.claude_text
             FROM turns t
             JOIN turns_fts ON turns_fts.rowid = t.id
             WHERE turns_fts MATCH ?
             ORDER BY bm25(turns_fts)
             LIMIT ?",
        )?;
        let rows = stmt.query_map(rusqlite::params![query, limit as i64], |row| {
            Ok(TurnRecord {
                id: row.get(0)?,
                ts: row.get(1)?,
                user_text: row.get(2)?,
                claude_text: row.get(3)?,
            })
        })?;
        let out: rusqlite::Result<Vec<TurnRecord>> = rows.collect();
        Ok(out?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Tmp path that doesn't collide across parallel test invocations or
    /// repeated runs. Uses the test thread id since `cargo test` parallelizes.
    fn tmp_db_path(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "aegis-history-test-{}-{}-{:?}.db",
            label,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn opens_and_creates_turns_table() {
        let path = tmp_db_path("schema");

        let store = HistoryStore::open(path.clone()).expect("open");

        // Re-open the same db with a fresh connection to confirm the
        // schema persisted to disk (not just held in the open conn).
        let conn = Connection::open(&store.path).expect("reopen");

        let table_exists: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type='table' AND name='turns'",
                [],
                |row| row.get(0),
            )
            .expect("query sqlite_master");
        assert_eq!(table_exists, 1, "turns table should exist after open");

        // Schema sanity: the four columns we expect, in the right order.
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('turns') ORDER BY cid")
            .expect("prepare pragma");
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query pragma")
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            cols,
            vec!["id", "ts", "user_text", "claude_text"],
            "turns table columns should match the bare schema"
        );

        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("query journal_mode");
        assert_eq!(
            journal_mode.to_lowercase(),
            "wal",
            "WAL mode should be enabled"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_is_idempotent() {
        let path = tmp_db_path("idempotent");

        let _first = HistoryStore::open(path.clone()).expect("first open");
        // Second open against the same file should succeed without
        // tripping over the existing schema.
        let _second = HistoryStore::open(path.clone()).expect("second open");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn record_and_get_roundtrip() {
        let path = tmp_db_path("roundtrip");
        let store = HistoryStore::open(path.clone()).expect("open");

        // Insert two turns, confirm ids increment and reads return what
        // we wrote.
        let id1 = store
            .record(
                1_700_000_000,
                "what's the weather",
                "I don't have weather data, but I can tell you about seasons.",
            )
            .expect("record turn 1");
        let id2 = store
            .record(
                1_700_000_042,
                "remember my favorite color is blue",
                "Got it, I'll remember that.",
            )
            .expect("record turn 2");

        assert!(id1 > 0, "ids should be assigned by SQLite");
        assert_eq!(id2, id1 + 1, "AUTOINCREMENT should hand out sequential ids");

        let read = store.get_by_id(id1).expect("get id1").expect("found id1");
        assert_eq!(read.id, id1);
        assert_eq!(read.ts, 1_700_000_000);
        assert_eq!(read.user_text, "what's the weather");
        assert!(read.claude_text.starts_with("I don't have weather data"));

        let read2 = store.get_by_id(id2).expect("get id2").expect("found id2");
        assert_eq!(read2.ts, 1_700_000_042);
        assert_eq!(read2.user_text, "remember my favorite color is blue");

        // Missing id is Ok(None), not an error. Callers shouldn't have
        // to distinguish "no such row" from "I/O failure."
        let missing = store.get_by_id(999_999).expect("get missing");
        assert!(missing.is_none(), "absent rows should return None");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unix_now_is_reasonable() {
        // Smoke test: the helper should return a unix epoch in the
        // expected ballpark (sometime after 2020). Catches accidental
        // breakage like returning 0 or a Duration in millis.
        let now = unix_now();
        assert!(now > 1_577_836_800, "unix_now should be after Jan 2020");
        assert!(now < 4_102_444_800, "unix_now should be before Jan 2100");
    }

    #[test]
    fn fts_table_exists_after_open() {
        let path = tmp_db_path("fts-schema");
        let store = HistoryStore::open(path.clone()).expect("open");

        let conn = Connection::open(&store.path).expect("reopen");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE name='turns_fts' AND type='table'",
                [],
                |row| row.get(0),
            )
            .expect("query");
        // FTS5 virtual tables show up as type='table' in sqlite_master,
        // alongside the shadow tables FTS5 creates internally
        // (turns_fts_data, turns_fts_idx, etc.). One row for the
        // virtual table itself is enough to confirm it was created.
        assert_eq!(count, 1, "turns_fts virtual table should exist");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn search_finds_keyword_matches() {
        let path = tmp_db_path("fts-find");
        let store = HistoryStore::open(path.clone()).expect("open");

        // Three turns, only one of which mentions Tokyo.
        store
            .record(1, "I'm flying to Tokyo next month", "Sounds exciting.")
            .expect("record");
        store
            .record(2, "what's the weather like", "I don't have weather data.")
            .expect("record");
        store
            .record(3, "remember my favorite color is blue", "Got it.")
            .expect("record");

        let hits = store.search_keyword("tokyo", 10).expect("search");
        assert_eq!(hits.len(), 1, "exactly one turn mentions Tokyo");
        assert!(hits[0].user_text.contains("Tokyo"));

        // A query that doesn't match anything returns an empty Vec,
        // not an error.
        let empty = store.search_keyword("xenomorph", 10).expect("empty search");
        assert!(empty.is_empty(), "no matches → empty Vec");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn search_matches_either_column() {
        let path = tmp_db_path("fts-cols");
        let store = HistoryStore::open(path.clone()).expect("open");

        // Match in user_text only.
        store
            .record(1, "tell me about Paris", "It's the capital of France.")
            .expect("record");
        // Match in claude_text only.
        store
            .record(
                2,
                "what's a good vacation spot",
                "Paris is lovely in spring.",
            )
            .expect("record");
        // No match.
        store
            .record(3, "remind me to buy milk", "Noted.")
            .expect("record");

        let hits = store.search_keyword("Paris", 10).expect("search");
        assert_eq!(hits.len(), 2, "FTS should match Paris in either column");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn search_supports_multi_word_query() {
        let path = tmp_db_path("fts-multi");
        let store = HistoryStore::open(path.clone()).expect("open");

        // Only the first turn mentions BOTH tokyo and flight.
        store
            .record(1, "book a flight to Tokyo", "Looking up flights now.")
            .expect("record");
        store
            .record(2, "I love Tokyo street food", "Ramen is great.")
            .expect("record");
        store
            .record(
                3,
                "flight prices are wild lately",
                "True, inflation is real.",
            )
            .expect("record");

        // FTS5 default behavior: space-separated terms are ANDed.
        let hits = store
            .search_keyword("tokyo flight", 10)
            .expect("multi search");
        assert_eq!(
            hits.len(),
            1,
            "AND query should only match the turn containing both words"
        );
        assert_eq!(hits[0].id, 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn search_respects_limit() {
        let path = tmp_db_path("fts-limit");
        let store = HistoryStore::open(path.clone()).expect("open");

        for i in 0..5 {
            store
                .record(i, "tokyo trip ideas", "Some thoughts about Tokyo.")
                .expect("record");
        }

        let hits = store.search_keyword("tokyo", 3).expect("search");
        assert_eq!(hits.len(), 3, "LIMIT should cap results");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn record_populates_fts_via_trigger() {
        // Confirms the INSERT trigger actually wires `turns` rows
        // into `turns_fts`. If the trigger is broken, records would
        // exist in turns but be invisible to search_keyword.
        let path = tmp_db_path("fts-trigger");
        let store = HistoryStore::open(path.clone()).expect("open");

        let id = store
            .record(1, "remember the alamo", "Noted, the alamo.")
            .expect("record");

        let hits = store.search_keyword("alamo", 10).expect("search");
        assert_eq!(hits.len(), 1, "trigger should have populated FTS index");
        assert_eq!(hits[0].id, id);

        let _ = std::fs::remove_file(&path);
    }
}
