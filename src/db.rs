use camino::{Utf8Path as Path, Utf8PathBuf as PathBuf};
use std::fs;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, ToSql};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SearchType {
    Simple,
    Match,
    Glob,
    #[cfg(feature = "regex")]
    Regex,
}

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

        #[cfg(feature = "regex")]
        register_regex(&conn).unwrap();
        register_glob(&conn).unwrap();

        let user_version: i32 = conn
            .query_row("SELECT user_version FROM pragma_user_version", [], |row| {
                row.get(0)
            })
            .unwrap();

        let db = DB { conn };
        match user_version {
            0 => db.init_db()?,
            1 => panic!(
                "Your database is from a prerelease version and should be deleted, its at {}",
                path
            ),
            2 => (),
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
                content TEXT NOT NULL
            );
            CREATE INDEX mark_delete_idx ON images (mark_delete);
            -- we use external-content fts because otherwise I got strange consistency errors
            CREATE VIRTUAL TABLE images_fts USING fts5(content, content=images, content_rowid=id, tokenize='trigram case_sensitive 0');
            CREATE TRIGGER images_insert AFTER INSERT ON images BEGIN
                INSERT INTO images_fts (rowid, content) VALUES (new.id, new.content);
            END;
            CREATE TRIGGER images_delete AFTER DELETE ON images BEGIN
                INSERT INTO images_fts (images_fts, rowid, content) VALUES ('delete', old.id, old.content);
            END;
            CREATE TRIGGER images_update AFTER UPDATE ON images BEGIN
                INSERT INTO images_fts (images_fts, rowid, content) VALUES ('delete', old.id, old.content);
                INSERT INTO images_fts (rowid, content) VALUES (new.id, new.content);
            END;
            PRAGMA user_version = 2;
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
            let mut index_stmt = tx
                .prepare_cached("INSERT INTO images (path, modtime, content) VALUES (?1, ?2, ?3) ON CONFLICT(path) DO UPDATE SET modtime=excluded.modtime, content=excluded.content")
                .unwrap();
            results
                .into_iter()
                .map(|res| {
                    index_stmt
                        .execute((
                            res.path.as_str(),
                            metadata_to_seconds(&res.metadata),
                            res.contents,
                        ))
                        .with_context(|| format!("failed to insert image: {}", res.path))
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
        kind: SearchType,
        exclude_glob: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let query = if kind == SearchType::Simple {
            format!(r#""{}""#, queries.join(" ").replace('*', "\\*"))
        } else {
            queries.join(" ")
        };

        let mut stmt = self
            .conn
            .prepare_cached(
                &format!(r#"
                SELECT snippet(images_fts, -1, '[', ']', '..', 64), images.path, images.modtime
                    FROM images_fts
                    INNER JOIN images ON images_fts.rowid = images.id AND images.path LIKE ?2 ESCAPE '#'
                    WHERE images_fts.content {kind} ?1 {exclude}
                    ORDER BY RANK, images.modtime DESC
                    LIMIT ?3;
                "#, kind=match kind {
                    SearchType::Simple | SearchType::Match => "MATCH",
                    SearchType::Glob => "GLOB",
                    #[cfg(feature="regex")]
                    SearchType::Regex => "REGEXP"
                }, exclude=if exclude_glob.is_some() {"AND NOT rust_glob(?4||'/**', images.path)"} else {""}),
            )
            .unwrap();
        let fixed_path = path_to_like(path);
        let mut params = vec![
            &query as &dyn ToSql,
            &fixed_path as &dyn ToSql,
            &limit as &dyn ToSql,
        ];
        if exclude_glob.is_some() {
            params.push(&exclude_glob as &dyn ToSql);
        }
        let results = stmt
            .query_and_then(params.as_slice(), |row| {
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

#[cfg(feature = "regex")]
fn register_regex(db: &Connection) -> Result<()> {
    use regex::Regex;
    use rusqlite::functions::FunctionFlags;
    use std::sync::Arc;
    db.create_scalar_function(
        "regexp",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            assert_eq!(ctx.len(), 2, "called with unexpected number of arguments");
            let regexp: Arc<Regex> =
                ctx.get_or_create_aux(0, |vr| -> Result<_> { Ok(Regex::new(vr.as_str()?)?) })?;
            let is_match = {
                let text = ctx
                    .get_raw(1)
                    .as_str()
                    .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))?;

                regexp.is_match(text)
            };

            Ok(is_match)
        },
    )?;
    Ok(())
}

fn register_glob(db: &Connection) -> Result<()> {
    use glob::Pattern;
    use rusqlite::functions::FunctionFlags;
    use std::sync::Arc;
    db.create_scalar_function(
        "rust_glob",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            assert_eq!(ctx.len(), 2, "called with unexpected number of arguments");
            let pattern: Arc<Pattern> =
                ctx.get_or_create_aux(0, |vr| -> Result<_> { Ok(Pattern::new(vr.as_str()?)?) })?;
            let is_match = {
                let text = ctx
                    .get_raw(1)
                    .as_str()
                    .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))?;

                pattern.matches(text)
            };

            Ok(is_match)
        },
    )?;
    Ok(())
}

#[cfg(test)]
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
                path: PathBuf::try_from(temp.path().join(contents.replace(' ', "_"))).unwrap(),
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
        let results = db.search(vec!["needle"], Path::new("/"), 40, SearchType::Simple, None)?;
        println!("{:?}", results);
        assert_eq!(results.len(), 2);

        temp.close()?;
        Ok(())
    }
}
