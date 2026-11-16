//! SQLite metadata store (`/local/nothingdatabase/nothingda.db`).
//!
//! One row per *version*, not per package: two projects can need axios 2.3.4
//! and axios 2.8.6 at the same time and the store keeps both zips. Exactly
//! one version per package is `active` — the one the most recent install
//! put there — and that row's `prev_version` is the rollback step that
//! `nth clear <pkg> new` uses.
//!
//! package.json is the source of truth for ranges; this db is the record of
//! what the machine actually holds.

use crate::cli::Source;
use crate::json::Json;
use rusqlite::{params, Connection};
use std::path::Path;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS packages (
  name         TEXT NOT NULL,
  version      TEXT NOT NULL,
  prev_version TEXT,
  source       TEXT NOT NULL DEFAULT 'npm',
  size_kb      INTEGER,
  beta         INTEGER NOT NULL DEFAULT 0,
  checksum     TEXT,
  tarball      TEXT,
  integrity    TEXT,
  active       INTEGER NOT NULL DEFAULT 1,
  deps         TEXT NOT NULL DEFAULT '[]',
  installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  updated_at   DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (name, version)
);
CREATE INDEX IF NOT EXISTS idx_packages_name ON packages(name, version);
"#;

#[derive(Clone, Debug)]
pub struct StoredDep {
    pub name: String,
    pub range: String,
    pub version: Option<String>,
    pub source: Source,
    pub checksum: Option<String>,
    /// Optional dependencies may legitimately be missing (another platform,
    /// no network); the resolver must not fail over them.
    pub optional: bool,
}

#[derive(Clone, Debug)]
pub struct StoredPackage {
    pub name: String,
    pub version: String,
    pub prev_version: Option<String>,
    pub source: Source,
    pub size_kb: u64,
    pub beta: bool,
    pub checksum: Option<String>,
    /// Where this version came from, so a lockfile can carry download URLs.
    pub tarball: Option<String>,
    pub integrity: Option<String>,
    /// The version the last install selected for this package name.
    pub active: bool,
    pub deps: Vec<StoredDep>,
}

const SELECT_COLUMNS: &str =
    "name, version, prev_version, source, size_kb, beta, checksum, deps, tarball, integrity, active";

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let conn = Connection::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("cannot configure database: {e}"))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("cannot create schema: {e}"))?;
        migrate_single_version(&conn)?;
        Ok(Db { conn })
    }

    /// The active version of a package, if the store has one.
    pub fn get(&self, name: &str) -> Result<Option<StoredPackage>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM packages WHERE name = ?1 AND active = 1"
            ))
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query(params![name]).map_err(|e| e.to_string())?;
        match rows.next().map_err(|e| e.to_string())? {
            Some(row) => Ok(Some(row_to_package(row)?)),
            None => Ok(None),
        }
    }

    /// Every version of a package the store holds.
    pub fn versions(&self, name: &str) -> Result<Vec<StoredPackage>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM packages WHERE name = ?1 ORDER BY installed_at"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![name], |row| Ok(row_to_package(row)))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())??);
        }
        Ok(out)
    }

    pub fn has(&self, name: &str, version: &str) -> Result<bool, String> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM packages WHERE name = ?1 AND version = ?2",
                params![name, version],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(count > 0)
    }

    /// Every row, newest install first, for `nth view db`.
    pub fn all(&self) -> Result<Vec<StoredPackage>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM packages ORDER BY name COLLATE NOCASE, installed_at"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok(row_to_package(row)))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())??);
        }
        Ok(out)
    }

    /// One row per package name: the active version. Commands that think in
    /// trees (`nth info`, `nth update`) use this.
    pub fn active_all(&self) -> Result<Vec<StoredPackage>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM packages WHERE active = 1 ORDER BY name COLLATE NOCASE"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok(row_to_package(row)))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())??);
        }
        Ok(out)
    }

    /// Records a version as active. Other versions of the same package stay
    /// in the store, they just stop being the active one.
    pub fn upsert(&self, pkg: &StoredPackage) -> Result<(), String> {
        let previous_active = self.get(&pkg.name)?;
        let prev = match &previous_active {
            Some(old) if old.version != pkg.version => Some(old.version.clone()),
            Some(old) => old.prev_version.clone(),
            None => None,
        };
        let deps_json = deps_to_json(&pkg.deps);

        self.conn
            .execute("UPDATE packages SET active = 0 WHERE name = ?1", params![pkg.name])
            .map_err(|e| e.to_string())?;
        self.conn
            .execute(
                "INSERT INTO packages
                   (name, version, prev_version, source, size_kb, beta, checksum, tarball, integrity, active, deps)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10)
                 ON CONFLICT(name, version) DO UPDATE SET
                   prev_version = excluded.prev_version,
                   source = excluded.source,
                   size_kb = excluded.size_kb,
                   beta = excluded.beta,
                   checksum = excluded.checksum,
                   tarball = COALESCE(excluded.tarball, packages.tarball),
                   integrity = COALESCE(excluded.integrity, packages.integrity),
                   active = 1,
                   deps = excluded.deps,
                   updated_at = CURRENT_TIMESTAMP",
                params![
                    pkg.name,
                    pkg.version,
                    prev,
                    pkg.source.as_str(),
                    pkg.size_kb as i64,
                    pkg.beta as i64,
                    pkg.checksum,
                    pkg.tarball,
                    pkg.integrity,
                    deps_json
                ],
            )
            .map_err(|e| format!("db write failed for {}: {e}", pkg.name))?;
        Ok(())
    }

    /// Used by `nth clear <pkg> new`: the previous version becomes active.
    pub fn promote_prev(&self, name: &str) -> Result<Option<String>, String> {
        let pkg = match self.get(name)? {
            Some(p) => p,
            None => return Ok(None),
        };
        let prev = match pkg.prev_version {
            Some(p) => p,
            None => return Ok(None),
        };
        self.conn
            .execute("UPDATE packages SET active = 0 WHERE name = ?1", params![name])
            .map_err(|e| e.to_string())?;
        self.conn
            .execute(
                "UPDATE packages SET active = 1, prev_version = NULL, updated_at = CURRENT_TIMESTAMP
                 WHERE name = ?1 AND version = ?2",
                params![name, prev],
            )
            .map_err(|e| e.to_string())?;
        Ok(Some(prev))
    }

    /// `nth clear <pkg> prev`: drop the rollback version. Returns it so the
    /// caller can remove that zip from the store.
    pub fn clear_prev(&self, name: &str) -> Result<Option<String>, String> {
        let prev = match self.get(name)? {
            Some(p) => p.prev_version,
            None => None,
        };
        let prev = match prev {
            Some(v) => v,
            None => return Ok(None),
        };
        self.conn
            .execute(
                "UPDATE packages SET prev_version = NULL WHERE name = ?1 AND active = 1",
                params![name],
            )
            .map_err(|e| e.to_string())?;
        self.delete_version(name, &prev)?;
        Ok(Some(prev))
    }

    pub fn delete_version(&self, name: &str, version: &str) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                "DELETE FROM packages WHERE name = ?1 AND version = ?2",
                params![name, version],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    /// Remove every version of a package. Returns the versions that were
    /// removed so the caller can delete their zips.
    pub fn delete_all(&self, name: &str) -> Result<Vec<String>, String> {
        let versions: Vec<String> = self
            .versions(name)?
            .into_iter()
            .map(|p| p.version)
            .collect();
        if versions.is_empty() {
            return Ok(versions);
        }
        self.conn
            .execute("DELETE FROM packages WHERE name = ?1", params![name])
            .map_err(|e| e.to_string())?;
        Ok(versions)
    }
}

/// 0.1.x stored one row per package in a table with `name TEXT UNIQUE` and no
/// `active` column. Rebuild that table with the composite key.
fn migrate_single_version(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "packages")?;
    if columns.is_empty() || columns.iter().any(|c| c == "active") {
        return Ok(());
    }
    let tarball = if columns.iter().any(|c| c == "tarball") {
        "tarball"
    } else {
        "NULL"
    };
    let integrity = if columns.iter().any(|c| c == "integrity") {
        "integrity"
    } else {
        "NULL"
    };

    conn.execute_batch("ALTER TABLE packages RENAME TO packages_legacy")
        .map_err(|e| format!("cannot migrate store db: {e}"))?;
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("cannot migrate store db: {e}"))?;
    conn.execute_batch(&format!(
        "INSERT OR IGNORE INTO packages
           (name, version, prev_version, source, size_kb, beta, checksum, tarball, integrity, active, deps, installed_at, updated_at)
         SELECT name, version, prev_version, source, size_kb, beta, checksum, {tarball}, {integrity}, 1, deps, installed_at, updated_at
         FROM packages_legacy;
         DROP TABLE packages_legacy;"
    ))
    .map_err(|e| format!("cannot migrate store db: {e}"))?;
    Ok(())
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn row_to_package(row: &rusqlite::Row<'_>) -> Result<StoredPackage, String> {
    let source_text: String = row.get(3).map_err(|e| e.to_string())?;
    let deps_text: String = row.get(7).map_err(|e| e.to_string())?;
    Ok(StoredPackage {
        name: row.get(0).map_err(|e| e.to_string())?,
        version: row.get(1).map_err(|e| e.to_string())?,
        prev_version: row.get(2).map_err(|e| e.to_string())?,
        source: Source::parse(&source_text).unwrap_or(Source::Npm),
        size_kb: row.get::<_, Option<i64>>(4).unwrap_or(None).unwrap_or(0).max(0) as u64,
        beta: row.get::<_, Option<i64>>(5).unwrap_or(None).unwrap_or(0) != 0,
        checksum: row.get(6).map_err(|e| e.to_string())?,
        tarball: row.get(8).map_err(|e| e.to_string())?,
        integrity: row.get(9).map_err(|e| e.to_string())?,
        active: row.get::<_, Option<i64>>(10).unwrap_or(None).unwrap_or(1) != 0,
        deps: deps_from_json(&deps_text),
    })
}

pub(crate) fn deps_to_json(deps: &[StoredDep]) -> String {
    let items: Vec<Json> = deps
        .iter()
        .map(|d| {
            Json::obj(vec![
                ("name", Json::s(&d.name)),
                ("range", Json::s(&d.range)),
                (
                    "version",
                    match &d.version {
                        Some(v) => Json::s(v),
                        None => Json::Null,
                    },
                ),
                ("source", Json::s(d.source.as_str())),
                (
                    "checksum",
                    match &d.checksum {
                        Some(c) => Json::s(c),
                        None => Json::Null,
                    },
                ),
                ("optional", Json::Bool(d.optional)),
            ])
        })
        .collect();
    Json::Arr(items).to_string()
}

pub(crate) fn deps_from_json(text: &str) -> Vec<StoredDep> {
    let parsed = match Json::parse(text) {
        Ok(j) => j,
        Err(_) => return Vec::new(),
    };
    let items = match parsed.as_arr() {
        Some(a) => a,
        None => return Vec::new(),
    };
    items
        .iter()
        .filter_map(|item| {
            Some(StoredDep {
                name: item.str_at("name")?.to_string(),
                range: item.str_at("range").unwrap_or("*").to_string(),
                version: item.str_at("version").map(|s| s.to_string()),
                source: item
                    .str_at("source")
                    .and_then(Source::parse)
                    .unwrap_or(Source::Npm),
                checksum: item.str_at("checksum").map(|s| s.to_string()),
                optional: matches!(item.get("optional"), Some(Json::Bool(true))),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (Db, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "nth-db-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nothingda.db");
        let db = Db::open(&path).unwrap();
        (db, dir)
    }

    fn pkg(v: &str) -> StoredPackage {
        StoredPackage {
            name: "axios".into(),
            version: v.into(),
            prev_version: None,
            source: Source::Npm,
            size_kb: 12,
            beta: false,
            checksum: Some("sha256:aa".into()),
            tarball: Some(format!("https://registry/axios-{v}.tgz")),
            integrity: Some("sha512-aa".into()),
            active: true,
            deps: vec![StoredDep {
                name: "follow-redirects".into(),
                range: "^1.15.0".into(),
                version: Some("1.15.6".into()),
                source: Source::Npm,
                checksum: None,
                optional: false,
            }],
        }
    }

    #[test]
    fn records_and_rolls_back_versions() {
        let (db, dir) = temp_db();
        db.upsert(&pkg("2.2.3")).unwrap();
        assert_eq!(db.get("axios").unwrap().unwrap().version, "2.2.3");

        db.upsert(&pkg("2.2.4")).unwrap();
        let stored = db.get("axios").unwrap().unwrap();
        assert_eq!(stored.version, "2.2.4");
        assert_eq!(stored.prev_version.as_deref(), Some("2.2.3"));
        assert_eq!(stored.tarball.as_deref(), Some("https://registry/axios-2.2.4.tgz"));
        assert_eq!(stored.integrity.as_deref(), Some("sha512-aa"));
        assert_eq!(stored.deps.len(), 1);
        assert!(!stored.deps[0].optional);

        assert_eq!(db.promote_prev("axios").unwrap().as_deref(), Some("2.2.3"));
        assert_eq!(db.get("axios").unwrap().unwrap().version, "2.2.3");
        assert!(db.delete_version("axios", "2.2.3").unwrap());
        assert!(db.get("axios").unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keeps_every_version_and_one_active() {
        let (db, dir) = temp_db();
        db.upsert(&pkg("1.4.5")).unwrap();
        db.upsert(&pkg("2.2.3")).unwrap();
        // A second project asking for the older version makes it active and
        // keeps the newer zip around for the project that wants it.
        db.upsert(&pkg("1.4.5")).unwrap();

        let versions = db.versions("axios").unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(db.get("axios").unwrap().unwrap().version, "1.4.5");
        assert!(db.has("axios", "2.2.3").unwrap());
        assert!(db.has("axios", "1.4.5").unwrap());
        assert!(!db.has("axios", "9.9.9").unwrap());

        let active = db.active_all().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].version, "1.4.5");
        assert_eq!(db.all().unwrap().len(), 2);

        let removed = db.delete_all("axios").unwrap();
        assert_eq!(removed.len(), 2);
        assert!(db.versions("axios").unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_prev_forgets_the_rollback_step() {
        let (db, dir) = temp_db();
        db.upsert(&pkg("1.4.5")).unwrap();
        db.upsert(&pkg("2.2.3")).unwrap();
        assert_eq!(db.clear_prev("axios").unwrap().as_deref(), Some("1.4.5"));
        assert!(db.get("axios").unwrap().unwrap().prev_version.is_none());
        assert!(!db.has("axios", "1.4.5").unwrap());
        assert!(db.clear_prev("axios").unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrates_a_single_version_table() {
        let dir = std::env::temp_dir().join(format!(
            "nth-db-migrate-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nothingda.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE packages (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   name TEXT NOT NULL UNIQUE,
                   version TEXT NOT NULL,
                   prev_version TEXT,
                   source TEXT NOT NULL DEFAULT 'npm',
                   size_kb INTEGER,
                   beta INTEGER NOT NULL DEFAULT 0,
                   checksum TEXT,
                   deps TEXT NOT NULL DEFAULT '[]',
                   installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                   updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
                 );
                 INSERT INTO packages (name, version, checksum, deps) VALUES ('axios', '2.2.3', 'sha256:aa', '[]');",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let stored = db.get("axios").unwrap().unwrap();
        assert_eq!(stored.version, "2.2.3");
        assert!(stored.active);
        let _ = std::fs::remove_dir_all(dir);
    }
}
