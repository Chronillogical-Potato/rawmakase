-- Every table of a RAWmakase catalog (SQLite application id 0x4f4d4152,
-- user_version 1). Idempotent: `Catalog::create` runs it on a new file and
-- `Catalog::open` on every open, so a catalog from an earlier release gains the
-- tables added since. Tables are only ever added; a change to an existing one
-- needs a new user_version and a migration.

CREATE TABLE IF NOT EXISTS sources (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    original_size INTEGER NOT NULL,
    original_catalog BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS roots (
    id INTEGER PRIMARY KEY,
    original_path TEXT NOT NULL,
    mapped_path TEXT
);

CREATE TABLE IF NOT EXISTS folders (
    id INTEGER PRIMARY KEY,
    root INTEGER NOT NULL REFERENCES roots(id),
    relative_path TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS photos (
    id INTEGER PRIMARY KEY,
    folder INTEGER NOT NULL REFERENCES folders(id),
    filename TEXT NOT NULL,
    original_path TEXT NOT NULL,
    captured TEXT NOT NULL DEFAULT '',
    rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5),
    flag INTEGER NOT NULL DEFAULT 0 CHECK(flag BETWEEN -1 AND 1),
    label TEXT NOT NULL DEFAULT '',
    format TEXT NOT NULL DEFAULT '',
    copy_name TEXT NOT NULL DEFAULT '',
    master_id INTEGER,
    orientation TEXT,
    lightroom_develop TEXT,
    recipe TEXT,
    export_options TEXT,
    identity TEXT,
    edited_at TEXT
);

CREATE INDEX IF NOT EXISTS photos_folder ON photos(folder);

CREATE INDEX IF NOT EXISTS photos_captured ON photos(captured);

CREATE INDEX IF NOT EXISTS photos_master ON photos(master_id);

CREATE TABLE IF NOT EXISTS collections (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent INTEGER,
    kind TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS collection_photos (
    collection INTEGER NOT NULL REFERENCES collections(id),
    photo INTEGER NOT NULL REFERENCES photos(id),
    position TEXT,
    PRIMARY KEY(collection,photo)
);

CREATE TABLE IF NOT EXISTS keywords (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent INTEGER
);

CREATE TABLE IF NOT EXISTS photo_keywords (
    photo INTEGER NOT NULL REFERENCES photos(id),
    keyword INTEGER NOT NULL REFERENCES keywords(id),
    PRIMARY KEY(photo,keyword)
);

CREATE TABLE IF NOT EXISTS folder_mappings (
    folder INTEGER PRIMARY KEY REFERENCES folders(id),
    path TEXT NOT NULL
);

-- Added after version 1 shipped.

-- Lightroom's develop history per photo: one full settings snapshot per step.
CREATE TABLE IF NOT EXISTS lightroom_history (
    photo INTEGER NOT NULL,
    position INTEGER NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    created REAL,
    text TEXT NOT NULL,
    PRIMARY KEY(photo, position)
);

-- Spots and masks of a photo's edit (experimental), as `LocalEdits` JSON: kept out
-- of the recipe column so releases before them still read every edit.
CREATE TABLE IF NOT EXISTS local_edits (
    photo INTEGER PRIMARY KEY,
    data TEXT NOT NULL
);

-- Compressed bitmaps referenced by hash from saved recipes (see `storage::bitmaps`).
CREATE TABLE IF NOT EXISTS bitmaps (
    hash TEXT PRIMARY KEY,
    data BLOB NOT NULL
);

-- Camera settings and size of a photo, from the Lightroom catalog it came from
-- or read from its file; a row of NULLs records a file that had none.
CREATE TABLE IF NOT EXISTS photo_info (
    photo INTEGER PRIMARY KEY,
    camera TEXT,
    lens TEXT,
    focal REAL,
    aperture REAL,
    exposure REAL,
    iso REAL,
    width INTEGER,
    height INTEGER
);

-- Facts about the catalog itself, by name.
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
