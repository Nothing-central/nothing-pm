//! SQLite metadata store (`/local/nothingdatabase/nothingda.db`).
//!
//! package.json is the source of truth for versions; this db is the only
//! record of what the machine actually holds — which version is active, what
//! the previous version was, where each package came from, its checksum and
//! its resolved dependency list (which is what `nth info` reads).

use crate::cli::Source;
use crate::json::Json;
use rusqlite::{params, Connection};
use std::path::Path;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS packages (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  name         TEXT NOT NULL UNIQUE,
  version      TEXT NOT NULL,
  prev_version TEXT,
  source       TEXT NOT NULL DEFAULT 'npm',
  size_kb      INTEGER,
  beta         INTEGER NOT NULL DEFAULT 0,
  checksum     TEXT,
  tarball      TEXT,
  integrity    TEXT,
  deps         TEXT NOT NULL DEFAULT '[]',
  installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  updated_at   DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_packages_name ON packages(name);
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
    pub deps: Vec<StoredDep>,
}

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
        // Stores created before 0.2.0 have no download URLs: add the columns
        // in place ("duplicate column" is the expected error on new stores).
        for column in ["tarball", "integrity"] {
            let _ = conn.execute(
                &format!("ALTER TABLE packages ADD COLUMN {column} TEXT"),
                [],
            );
        }
        Ok(Db { conn })
    }

    pub fn get(&self, name: &str) -> Result<Option<StoredPackage>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, version, prev_version, source, size_kb, beta, checksum, deps, tarball, integrity
                 FROM packages WHERE name = ?1",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query(params![name]).map_err(|e| e.to_string())?;
        match rows.next().map_err(|e| e.to_string())? {
            Some(row) => Ok(Some(row_to_package(row)?)),
            None => Ok(None),
        }
    }

    pub fn all(&self) -> Result<Vec<StoredPackage>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, version, prev_version, source, size_kb, beta, checksum, deps, tarball, integrity
                 FROM packages ORDER BY name COLLATE NOCASE",
            )
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

    /// Records a package as active. If a different version was already
    /// stored, it becomes `prev_version` (the store keeps exactly one
    /// rollback step).
    pub fn upsert(&self, pkg: &StoredPackage) -> Result<(), String> {
        let existing = self.get(&pkg.name)?;
        let (prev, installed_at) = match &existing {
            Some(old) if old.version != pkg.version => (
                Some(old.version.clone()),
                current_installed_at(&self.conn, &pkg.name)?,
            ),
            Some(old) => (old.prev_version.clone(), None),
            None => (None, None),
        };
        let deps_json = deps_to_json(&pkg.deps);

        self.conn
            .execute(
                "INSERT INTO packages (name, version, prev_version, source, size_kb, beta, checksum, deps, tarball, integrity)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(name) DO UPDATE SET
                   version = excluded.version,
                   prev_version = excluded.prev_version,
                   source = excluded.source,
                   size_kb = excluded.size_kb,
                   beta = excluded.beta,
                   checksum = excluded.checksum,
                   deps = excluded.deps,
                   tarball = excluded.tarball,
                   integrity = excluded.integrity,
                   updated_at = CURRENT_TIMESTAMP",
                params![
                    pkg.name,
                    pkg.version,
                    prev,
                    pkg.source.as_str(),
                    pkg.size_kb as i64,
                    pkg.beta as i64,
                    pkg.checksum,
                    deps_json,
                    pkg.tarball,
                    pkg.integrity
                ],
            )
            .map_err(|e| format!("db write failed for {}: {e}", pkg.name))?;

        if let Some(ts) = installed_at {
            // Preserve the original install date across updates.
            self.conn
                .execute(
                    "UPDATE packages SET installed_at = ?1 WHERE name = ?2",
                    params![ts, pkg.name],
                )
                .map_err(|e| e.to_string())?;
        }
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
            .execute(
                "UPDATE packages SET version = ?1, prev_version = NULL, updated_at = CURRENT_TIMESTAMP
                 WHERE name = ?2",
                params![prev, name],
            )
            .map_err(|e| e.to_string())?;
        Ok(Some(prev))
    }

    /// `nth clear <pkg> prev`: forget the rollback version.
    pub fn clear_prev(&self, name: &str) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                "UPDATE packages SET prev_version = NULL WHERE name = ?1 AND prev_version IS NOT NULL",
                params![name],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    pub fn delete(&self, name: &str) -> Result<bool, String> {
        let changed = self
            .conn
            .execute("DELETE FROM packages WHERE name = ?1", params![name])
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }
}

fn current_installed_at(conn: &Connection, name: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT installed_at FROM packages WHERE name = ?1",
        params![name],
        |row| row.get::<_, Option<String>>(0),
    )
    .map_err(|e| e.to_string())
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
        let dir = std::env::temp_dir().join(format!("nth-db-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nothingda.db");
        let db = Db::open(&path).unwrap();
        (db, dir)
    }

    #[test]
    fn records_and_rolls_back_versions() {
        let (db, dir) = temp_db();
        let pkg = |v: &str| StoredPackage {
            name: "axios".into(),
            version: v.into(),
            prev_version: None,
            source: Source::Npm,
            size_kb: 12,
            beta: false,
            checksum: Some("sha256:aa".into()),
            tarball: Some("https://registry/axios.tgz".into()),
            integrity: Some("sha512-aa".into()),
            deps: vec![StoredDep {
                name: "follow-redirects".into(),
                range: "^1.15.0".into(),
                version: Some("1.15.6".into()),
                source: Source::Npm,
                checksum: None,
                optional: false,
            }],
        };
        db.upsert(&pkg("2.2.3")).unwrap();
        assert_eq!(db.get("axios").unwrap().unwrap().version, "2.2.3");

        db.upsert(&pkg("2.2.4")).unwrap();
        let stored = db.get("axios").unwrap().unwrap();
        assert_eq!(stored.version, "2.2.4");
        assert_eq!(stored.prev_version.as_deref(), Some("2.2.3"));
        assert_eq!(stored.tarball.as_deref(), Some("https://registry/axios.tgz"));
        assert_eq!(stored.integrity.as_deref(), Some("sha512-aa"));
        assert_eq!(stored.deps.len(), 1);
        assert_eq!(stored.deps[0].name, "follow-redirects");
        assert!(!stored.deps[0].optional);

        assert_eq!(db.promote_prev("axios").unwrap().as_deref(), Some("2.2.3"));
        assert_eq!(db.get("axios").unwrap().unwrap().version, "2.2.3");
        assert!(db.delete("axios").unwrap());
        assert!(db.get("axios").unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
