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