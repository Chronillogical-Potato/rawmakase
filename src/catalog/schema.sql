PRAGMA foreign_keys=ON;

CREATE TABLE sources (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    original_size INTEGER NOT NULL,
    original_catalog BLOB NOT NULL
);

CREATE TABLE roots (
    id INTEGER PRIMARY KEY,
    original_path TEXT NOT NULL,
    mapped_path TEXT
);

CREATE TABLE folders (
    id INTEGER PRIMARY KEY,
    root INTEGER NOT NULL REFERENCES roots(id),
    relative_path TEXT NOT NULL
);

CREATE TABLE photos (
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

CREATE INDEX photos_folder ON photos(folder);

CREATE INDEX photos_captured ON photos(captured);

CREATE TABLE collections (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent INTEGER,
    kind TEXT NOT NULL
);

CREATE TABLE collection_photos (
    collection INTEGER NOT NULL REFERENCES collections(id),
    photo INTEGER NOT NULL REFERENCES photos(id),
    position TEXT,
    PRIMARY KEY(collection,photo)
);

CREATE TABLE keywords (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent INTEGER
);

CREATE TABLE photo_keywords (
    photo INTEGER NOT NULL REFERENCES photos(id),
    keyword INTEGER NOT NULL REFERENCES keywords(id),
    PRIMARY KEY(photo,keyword)
);

CREATE TABLE folder_mappings (
    folder INTEGER PRIMARY KEY REFERENCES folders(id),
    path TEXT NOT NULL
);
