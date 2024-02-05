use camino::{Utf8Path as Path, Utf8PathBuf as PathBuf};
use std::fs;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub struct DB {
    conn: Connection,
}

impl DB {
    pub fn new(path: &Path) -> Result<Self> {
        if !path.try_exists()? {
            eprintln!("Note: creating new database")
        }
        let conn = Connection::open(path)?;

        conn.pragma_update(None, "journal_mode", "wal").unwrap();
        conn.pragma_update(None, "synchronous", "normal").unwrap(); // TODO: maybe not

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
                mark_delete BOOL DEFAULT FALSE
            );
            CREATE INDEX mark_delete_idx ON images (mark_delete);
            CREATE VIRTUAL TABLE images_fts USING fts5(result, tokenize='trigram case_sensitive 1');
            PRAGMA user_version = 1;
            COMMIT;
            "#,
        )
        .context("creating tables")?;

        Ok(())
    }

    pub fn is_indexed(&self, path: &Path, metadata: &fs::Metadata) -> bool {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT modtime FROM images WHERE path = ?1")
            .unwrap();
        let mtime = metadata_to_seconds(metadata);
        let Some(modtime) = stmt
            .query_row([path.as_str()], |row| row.get(0))
            .optional()
            .with_context(|| format!("failed to check if an image was already indexed: {}", path))
            .unwrap()
        else {
            return false;
        };
        if mtime == modtime {
            return true;
        }
        false
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
                    let rowid = metadata_stmt
                        .query_row(
                            (res.path.as_str(), metadata_to_seconds(&res.metadata)),
                            |row| row.get::<_, i64>(0),
                        )
                        .with_context(|| format!("failed to insert metadata: {}", res.path))
                        .unwrap();
                    (rowid, res.contents)
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(id, contents)| {
                    fts_stmt
                        .execute((id, &contents))
                        .with_context(|| {
                            format!("failed to index ocr result: {:?}", (id, &contents))
                        })
                        .unwrap()
                })
                .sum()
        };
        tx.commit().unwrap();
        Ok(rowchanges)
    }

    /// Mark the elements of a directory for deletion in the DB
    pub fn mark_for_deletion(&mut self, path: &Path) {
        if !path.is_dir() {
            panic!(
                "Incorrect usage of `mark_for_deletion`: Path `{}` should be a directory",
                path
            );
        }

        self.conn
            .execute(
                "UPDATE images SET mark_delete = FALSE WHERE mark_delete = TRUE",
                [],
            )
            .expect("failed to preliminarily unmark previously marked images for deletion");
        let mut stmt = self
            .conn
            .prepare_cached("UPDATE images SET mark_delete = TRUE WHERE path LIKE ?1 ESCAPE '#'")
            .expect("failed to preliminarily mark subdirectory for deletion");

        stmt.execute([path_to_like(path)]).unwrap();
    }

    pub fn unmark_file(&mut self, path: &Path) {
        let mut stmt = self
            .conn
            .prepare_cached("UPDATE images SET mark_delete = FALSE WHERE path = ?1")
            .with_context(|| format!("failed to unmark image for deletion: {}", path))
            .unwrap();

        stmt.execute([path.as_str()]).unwrap();
    }

    pub fn sweep_deletions(&mut self) -> usize {
        self.conn
            .execute("DELETE FROM images WHERE mark_delete = TRUE", [])
            .expect("failed to delete marked images")
    }

    pub fn search(
        &mut self,
        queries: Vec<&str>,
        path: &Path,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let query = format!(r#""{}""#, queries.join(" ").escape_default()); // TODO: support complex queries

        let mut stmt = self
            .conn
            .prepare_cached(
                r#"
                SELECT snippet(images_fts, -1, '[', ']', '..', 64), images.path, images.modtime
                    FROM images_fts
                    INNER JOIN images ON images_fts.rowid = images.id AND images.path LIKE ?2 ESCAPE '#'
                    WHERE images_fts.result MATCH ?1 ORDER BY RANK, images.modtime DESC
                    LIMIT ?3;
                "#,
            )
            .unwrap();
        let results = stmt
            .query_and_then(params![query, path_to_like(path), limit], |row| {
                Ok(SearchResult {
                    contents: row.get(0)?,
                    path: row.get(1)?,
                    time: row.get(2)?,
                })
            })
            .context("failed to query image index")?;
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
        .expect("unable to get file time")
        .duration_since(UNIX_EPOCH)
        .expect("duration should be after unix epoch")
        .as_secs()
}

fn path_to_like(s: &Path) -> String {
    let s = s.as_str();
    format!(
        "{}%",
        s.replace('#', "##").replace('%', "#%").replace('_', "#_")
    )
}

#[cfg(test)]
#[cfg(never)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::TempDir;

    fn test_db() -> Result<(TempDir, DB)> {
        let temp = TempDir::new()?;
        let db = DB::new(&PathBuf::try_from(temp.path().join("temp.db"))?)?;
        Ok((temp, db))
    }

    #[test]
    fn is_indexed() -> Result<()> {
        let (temp, mut db) = test_db()?;
        let dummy = PathBuf::try_from(temp.path().join("dummy"))?;
        File::create(&dummy)?;
        let dummy_metadata = fs::metadata(&dummy).unwrap();
        db.save_results(vec![OcrResult {
            path: dummy.clone(),
            metadata: dummy_metadata.clone(),
            contents: "nothing".into(),
        }])?;
        assert!(db.is_indexed(&dummy, &dummy_metadata));
        temp.close()?;
        Ok(())
    }

    #[test]
    fn deletion() -> Result<()> {
        let (temp, mut db) = test_db()?;
        let not_deleted = PathBuf::try_from(temp.path().join("im_not_going_away"))?;
        let deleted = PathBuf::try_from(temp.path().join("im_going_away"))?;
        File::create(&not_deleted)?;
        File::create(&deleted)?;
        db.save_results(vec![
            OcrResult {
                metadata: fs::metadata(&not_deleted)?,
                path: not_deleted.clone(),
                contents: "".into(),
            },
            OcrResult {
                metadata: fs::metadata(&deleted)?,
                path: deleted.clone(),
                contents: "".into(),
            },
        ])?;
        assert_eq!(db.sweep_deletions(), 0);
        db.mark_for_deletion(Path::from_path(temp.path()).unwrap());
        db.unmark_file(&not_deleted);
        assert_eq!(db.sweep_deletions(), 1);

        temp.close()?;
        Ok(())
    }

    #[test]
    fn search() -> Result<()> {
        let (temp, mut db) = test_db()?;
        let mock_metadata = fs::metadata(".").unwrap();
        let x = |contents: &'static str| -> OcrResult {
            OcrResult {
                path: PathBuf::try_from(temp.path().join(contents.replace(" ", "_"))).unwrap(),
                metadata: mock_metadata.clone(),
                contents: contents.into(),
            }
        };
        assert_eq!(
            db.save_results(vec![
                x("haystack haystack haystack"),
                x("haystack haystack needle"),
                x("haystack hayneedle haystack"),
            ])?,
            3
        );
        let results = db.search(vec!["needle"], Path::new("/"), 40)?;
        println!("{:?}", results);
        assert_eq!(results.len(), 2);

        temp.close()?;
        Ok(())
    }
}
