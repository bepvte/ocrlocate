use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use std::{env, fs};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

pub struct DB {
    conn: Connection,
}

impl DB {
    pub fn new(path: &Path) -> Result<Self> {
        if !path.try_exists()? {
            println!("Note: creating new database")
        }
        let conn = Connection::open(path)?;

        conn.pragma_update(None, "journal_mode", "wal").unwrap();
        conn.pragma_update(None, "synchronous", "normal")?; // TODO: maybe not

        let user_version: i32 = conn
            .query_row("SELECT user_version FROM pragma_user_version", [], |row| {
                row.get(0)
            })
            .unwrap();

        let db = DB { conn };
        match user_version {
            0 => db.init_db()?,
            1 => (),
            x => panic!("Database schema version is too high: {x}"),
        };

        Ok(db)
    }

    fn init_db(&self) -> Result<()> {
        let conn = &self.conn;

        conn.execute_batch(
            r#"
            BEGIN;
            CREATE TABLE images(
                id INTEGER PRIMARY KEY ASC,
                path TEXT UNIQUE NOT NULL,
                modtime INTEGER NOT NULL,
                mark_delete BOOL DEFAULT FALSE,
            );
            CREATE INDEX mark_delete_idx ON images (mark_delete);
            CREATE VIRTUAL TABLE images_fts USING fts5(result, tokenize='trigram case_sensitive 1');
            PRAGMA user_version = 1;
            COMMIT;
            "#,
        )
        .with_context(|| "creating tables")?;

        Ok(())
    }

    pub fn is_indexed(&self, path: &Path, metadata: &fs::Metadata) -> bool {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT modtime FROM images WHERE path = ?1")
            .unwrap();
        let mtime = metadata_to_seconds(metadata);
        let Some(modtime) = stmt
            .query_row(
                [path.to_str().expect("paths should be valid unicode")],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
        else {
            return false;
        };
        if mtime == modtime {
            return true;
        }
        return false;
    }

    pub fn save_results(&mut self, results: Vec<OcrResult>) -> Result<usize> {
        let tx = self.conn.transaction().unwrap();

        let rowchanges: usize = {
            let mut metadata_stmt = tx
                .prepare_cached("INSERT INTO images (path, modtime) VALUES (?1, ?2) RETURNING id")
                .unwrap();
            let mut fts_stmt = tx
                .prepare_cached("INSERT INTO images_fts (rowid, result) VALUES (?1, ?2)")
                .unwrap();

            results
                .into_iter()
                .map(|res| {
                    let path = res.path.to_str().expect("paths should be valid unicode");
                    let mtime = metadata_to_seconds(&res.metadata);
                    let rowid = metadata_stmt
                        .query_row((path, mtime), |row| row.get::<_, i64>(0))
                        .with_context(|| format!("metadata insertion: {:?}", (path, mtime)))
                        .unwrap();
                    (rowid, res.contents)
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(id, contents)| {
                    fts_stmt
                        .execute((id, &contents))
                        .with_context(|| format!("fts insertion: {:?}", (id, &contents)))
                        .unwrap()
                })
                .sum()
        };
        tx.commit().unwrap();
        Ok(rowchanges)
    }

    pub fn mark_for_deletion(&mut self, path: &Path) {
        self.conn
            .execute(
                "UPDATE images SET mark_delete = FALSE WHERE mark_delete = TRUE",
                [],
            )
            .unwrap();
        let mut stmt = self
            .conn
            .prepare_cached("UPDATE images SET mark_delete = TRUE WHERE path LIKE ?1")
            .unwrap();

        stmt.execute([path_to_like(path.to_str().unwrap())])
            .unwrap();
    }

    pub fn unmark_file(&mut self, path: &Path) {
        let mut stmt = self
            .conn
            .prepare_cached("UPDATE images SET mark_delete = FALSE WHERE path = ?1")
            .unwrap();

        stmt.execute([path.to_str().unwrap()]).unwrap();
    }

    pub fn sweep_deletions(&mut self) -> usize {
        self.conn
            .execute("DELETE FROM images WHERE mark_delete = TRUE", [])
            .unwrap()
    }

    pub fn search(&mut self, queries: Vec<&str>) -> Result<Vec<SearchResult>> {
        let query = format!(r#""{}""#, queries.join(" ").escape_default()); // TODO: support complex queries

        let mut stmt = self
            .conn
            .prepare_cached(
                r#"
                SELECT snippet(images_fts, -1, '[', ']', '..', 64), images.path, images.modtime
                    FROM images_fts
                    INNER JOIN images ON images_fts.rowid = images.id AND images.path LIKE ?2 ESCAPE "\"
                    WHERE images_fts.result MATCH ?1 ORDER BY RANK
                    LIMIT 50;
                "#,
            )
            .unwrap();
        let pwd = env::current_dir()
            .unwrap()
            .into_os_string()
            .into_string()
            .expect("path should be valid utf-8");
        let results = stmt.query_and_then([query, path_to_like(&pwd)], |row| {
            Ok(SearchResult {
                contents: row.get(0)?,
                path: row.get(1)?,
                time: row.get(2)?,
            })
        })?;
        results.collect()
    }
}

#[derive(Debug)]
pub struct OcrResult {
    pub path: PathBuf,
    pub metadata: fs::Metadata,
    pub contents: String,
}

#[derive(Debug)]
pub struct SearchResult {
    pub path: String,
    pub time: u64,
    pub contents: String,
}

fn metadata_to_seconds(m: &fs::Metadata) -> u64 {
    m.modified()
        .expect("system time shouldnt error")
        .duration_since(UNIX_EPOCH)
        .expect("duration should be after unix epoch")
        .as_secs()
}

fn path_to_like(s: &str) -> String {
    format!("{}%", s.replace("%", "\\%").replace("_", "\\_"))
}
