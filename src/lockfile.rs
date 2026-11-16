//! `agree.db` — the project lockfile, in SQLite form.
//!
//! package.json says which versions a project *wants*; `agree.db` records
//! which versions a project actually *got*, including every transitive
//! package, the checksum of what landed in the store and the tarball it came
//! from. It is a normal SQLite file, so `sqlite3 agree.db` works, and
//! `nth lock json` writes the same data as `agree.json` for people who want
//! to diff it in git or read it in a script.
//!
//! `nth i` reads the lockfile first: when the ranges in package.json still
//! match the ranges that produced the lock, the exact locked tree is
//! installed with no registry call at all.

use crate::cli::Source;
use crate::db::{self, Db, StoredDep};
use crate::json::Json;
use crate::resolver::{Plan, PlanItem, Root};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

pub const LOCK_NAME: &str = "agree.db";
pub const JSON_NAME: &str = "agree.json";

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS roots (
  name   TEXT PRIMARY KEY,
  range  TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'npm'
);
CREATE TABLE IF NOT EXISTS packages (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  name         TEXT NOT NULL,
  version      TEXT NOT NULL,
  source       TEXT NOT NULL DEFAULT 'npm',
  tarball      TEXT,
  integrity    TEXT,
  checksum     TEXT,
  beta         INTEGER NOT NULL DEFAULT 0,
  optional     INTEGER NOT NULL DEFAULT 0,
  nested_under TEXT,
  deps         TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS idx_lock_packages_name ON packages(name);
"#;

#[derive(Clone, Debug)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub source: Source,
    pub tarball: Option<String>,
    pub integrity: Option<String>,
    pub checksum: Option<String>,
    pub beta: bool,
    pub optional: bool,
    pub nested_under: Option<String>,
    pub deps: Vec<StoredDep>,
}

pub struct Lockfile {
    path: PathBuf,
    conn: Connection,
}

impl Lockfile {
    pub fn path_in(project: &Path) -> PathBuf {
        project.join(LOCK_NAME)
    }

    pub fn exists(project: &Path) -> bool {
        Self::path_in(project).is_file()
    }

    pub fn open(project: &Path) -> Result<Lockfile, String> {
        let path = Self::path_in(project);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let conn = Connection::open(&path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("cannot configure {}: {e}", path.display()))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("cannot create lockfile schema: {e}"))?;
        Ok(Lockfile { path, conn })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Replace the lockfile contents with `roots` + `plan`.
    pub fn write(&mut self, roots: &[Root], plan: &Plan) -> Result<usize, String> {
        self.conn
            .execute_batch("BEGIN")
            .map_err(|e| e.to_string())?;

        let written = (|| -> Result<usize, String> {
            self.conn
                .execute("DELETE FROM packages", [])
                .map_err(|e| e.to_string())?;
            self.conn
                .execute("DELETE FROM roots", [])
                .map_err(|e| e.to_string())?;

            for root in roots {
                self.conn
                    .execute(
                        "INSERT INTO roots (name, range, source) VALUES (?1, ?2, ?3)
                         ON CONFLICT(name) DO UPDATE SET range = excluded.range, source = excluded.source",
                        params![
                            root.name,
                            root.range.clone().unwrap_or_else(|| "*".into()),
                            root.source.as_str()
                        ],
                    )
                    .map_err(|e| format!("lockfile write failed for {}: {e}", root.name))?;
            }

            for item in &plan.items {
                self.conn
                    .execute(
                        "INSERT INTO packages
                           (name, version, source, tarball, integrity, checksum, beta, optional, nested_under, deps)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            item.name,
                            item.version,
                            item.source.as_str(),
                            item.tarball,
                            item.integrity,
                            item.checksum,
                            item.beta as i64,
                            item.optional as i64,
                            item.nested_under,
                            db::deps_to_json(&item.deps),
                        ],
                    )
                    .map_err(|e| format!("lockfile write failed for {}: {e}", item.name))?;
            }

            let meta = [
                ("lockfileVersion", "1".to_string()),
                ("generatedBy", format!("nth {}", env!("CARGO_PKG_VERSION"))),
            ];
            for (key, value) in meta {
                self.conn
                    .execute(
                        "INSERT INTO meta (key, value) VALUES (?1, ?2)
                         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                        params![key, value],
                    )
                    .map_err(|e| e.to_string())?;
            }

            Ok(plan.items.len())
        })();

        match written {
            Ok(count) => {
                self.conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                Ok(count)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    pub fn roots(&self) -> Result<Vec<(String, String, Source)>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, range, source FROM roots ORDER BY name COLLATE NOCASE")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let source: String = row.get(2)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    Source::parse(&source).unwrap_or(Source::Npm),
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn packages(&self) -> Result<Vec<LockedPackage>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, version, source, tarball, integrity, checksum, beta, optional, nested_under, deps
                 FROM packages ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let source: String = row.get(2)?;
                let deps_text: String = row.get(9)?;
                Ok(LockedPackage {
                    name: row.get(0)?,
                    version: row.get(1)?,
                    source: Source::parse(&source).unwrap_or(Source::Npm),
                    tarball: row.get(3)?,
                    integrity: row.get(4)?,
                    checksum: row.get(5)?,
                    beta: row.get::<_, i64>(6)? != 0,
                    optional: row.get::<_, i64>(7)? != 0,
                    nested_under: row.get(8)?,
                    deps: db::deps_from_json(&deps_text),
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// True when the lock was produced by exactly these roots. Any change to
    /// a range in package.json means the lock is stale and gets replanned.
    pub fn matches(&self, roots: &[Root]) -> Result<bool, String> {
        let locked = self.roots()?;
        if locked.len() != roots.len() {
            return Ok(false);
        }
        for root in roots {
            let wanted_range = root.range.clone().unwrap_or_else(|| "*".into());
            let found = locked.iter().any(|(name, range, source)| {
                name == &root.name && range == &wanted_range && *source == root.source
            });
            if !found {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Turn the lock back into a plan. `None` means the lock cannot be used
    /// on this machine (a package is missing from the store and the lock has
    /// no download URL for it), so the caller should resolve normally.
    pub fn to_plan(&self, db: &Db) -> Result<Option<Plan>, String> {
        let mut items = Vec::with_capacity(8);
        for pkg in self.packages()? {
            // The store keeps every version, so "is it here" is a lookup on
            // the exact (name, version) pair, not "is some version here".
            let in_store = db.has(&pkg.name, &pkg.version)?;
            items.push(PlanItem {
                name: pkg.name,
                version: pkg.version,
                source: pkg.source,
                tarball: pkg.tarball,
                integrity: pkg.integrity,
                beta: pkg.beta,
                checksum: pkg.checksum,
                deps: pkg.deps,
                in_store,
                nested_under: pkg.nested_under,
                optional: pkg.optional,
            });
        }
        Ok(Some(Plan::from_items(items)))
    }

    /// The same lockfile as json, ready to write to `agree.json`.
    pub fn to_json(&self) -> Result<String, String> {
        let mut roots: Vec<(String, Json)> = Vec::new();
        for (name, range, _source) in self.roots()? {
            roots.push((name, Json::s(&range)));
        }

        let mut packages: Vec<(String, Json)> = Vec::new();
        for pkg in self.packages()? {
            let key = match &pkg.nested_under {
                Some(parent) => format!("{parent}>{name}", name = pkg.name),
                None => pkg.name.clone(),
            };
            let mut deps: Vec<(String, Json)> = Vec::new();
            for dep in &pkg.deps {
                let mut fields = vec![("range", Json::s(&dep.range))];
                if let Some(version) = &dep.version {
                    fields.push(("version", Json::s(version)));
                }
                if dep.optional {
                    fields.push(("optional", Json::Bool(true)));
                }
                deps.push((dep.name.clone(), Json::obj(fields)));
            }

            packages.push((
                key,
                Json::obj(vec![
                    ("version", Json::s(&pkg.version)),
                    ("source", Json::s(pkg.source.as_str())),
                    (
                        "tarball",
                        pkg.tarball.clone().map(|t| Json::s(&t)).unwrap_or(Json::Null),
                    ),
                    (
                        "integrity",
                        pkg.integrity.clone().map(|i| Json::s(&i)).unwrap_or(Json::Null),
                    ),
                    (
                        "checksum",
                        pkg.checksum.clone().map(|c| Json::s(&c)).unwrap_or(Json::Null),
                    ),
                    ("optional", Json::Bool(pkg.optional)),
                    (
                        "nestedUnder",
                        pkg.nested_under
                            .clone()
                            .map(|n| Json::s(&n))
                            .unwrap_or(Json::Null),
                    ),
                    ("dependencies", Json::Obj(deps)),
                ]),
            ));
        }

        let doc = Json::obj(vec![
            ("lockfileVersion", Json::Num(1.0)),
            (
                "generatedBy",
                Json::s(&format!("nth {}", env!("CARGO_PKG_VERSION"))),
            ),
            ("roots", Json::Obj(roots)),
            ("packages", Json::Obj(packages)),
        ]);
        Ok(doc.to_pretty())
    }

    pub fn write_json(&self) -> Result<PathBuf, String> {
        let path = self
            .path
            .parent()
            .map(|dir| dir.join(JSON_NAME))
            .unwrap_or_else(|| PathBuf::from(JSON_NAME));
        std::fs::write(&path, self.to_json()?)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::StoredDep;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nth-lock-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_plan() -> Plan {
        let mut items = Vec::new();
        items.push(PlanItem {
            name: "axios".into(),
            version: "2.8.6".into(),
            source: Source::Npm,
            tarball: Some("https://registry/axios.tgz".into()),
            integrity: Some("sha512-aa".into()),
            beta: false,
            checksum: Some("sha256:11".into()),
            deps: vec![StoredDep {
                name: "follow-redirects".into(),
                range: "^1.15.0".into(),
                version: Some("1.15.6".into()),
                source: Source::Npm,
                checksum: Some("sha256:22".into()),
                optional: false,
            }],
            in_store: false,
            nested_under: None,
            optional: false,
        });
        items.push(PlanItem {
            name: "@esbuild/darwin-arm64".into(),
            version: "0.25.0".into(),
            source: Source::Npm,
            tarball: Some("https://registry/esbuild-darwin.tgz".into()),
            integrity: None,
            beta: false,
            checksum: None,
            deps: Vec::new(),
            in_store: false,
            nested_under: None,
            optional: true,
        });
        Plan::from_items(items)
    }

    fn roots() -> Vec<Root> {
        vec![Root {
            name: "axios".into(),
            range: Some("^2.8.6".into()),
            source: Source::Npm,
        }]
    }

    #[test]
    fn round_trips_a_plan_and_roots() {
        let dir = temp_dir();
        let plan = sample_plan();
        let mut lock = Lockfile::open(&dir).unwrap();
        assert_eq!(lock.write(&roots(), &plan).unwrap(), 2);

        assert!(lock.matches(&roots()).unwrap());
        let stale = vec![Root {
            name: "axios".into(),
            range: Some("^3.0.0".into()),
            source: Source::Npm,
        }];
        assert!(!lock.matches(&stale).unwrap());

        let packages = lock.packages().unwrap();
        assert_eq!(packages.len(), 2);
        assert!(packages[1].optional);
        assert_eq!(packages[0].deps[0].name, "follow-redirects");
        assert_eq!(packages[0].deps[0].version.as_deref(), Some("1.15.6"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exports_json_with_every_field() {
        let dir = temp_dir();
        let mut lock = Lockfile::open(&dir).unwrap();
        lock.write(&roots(), &sample_plan()).unwrap();
        let path = lock.write_json().unwrap();
        assert_eq!(path.file_name().unwrap(), JSON_NAME);

        let text = std::fs::read_to_string(&path).unwrap();
        let json = Json::parse(&text).unwrap();
        assert_eq!(
            json.get("roots").and_then(|r| r.str_at("axios")),
            Some("^2.8.6")
        );
        let packages = json.get("packages").and_then(|p| p.as_obj()).unwrap();
        let axios = packages
            .iter()
            .find(|(k, _)| k == "axios")
            .map(|(_, v)| v)
            .unwrap();
        assert_eq!(axios.str_at("version"), Some("2.8.6"));
        assert_eq!(axios.str_at("checksum"), Some("sha256:11"));
        assert_eq!(
            axios
                .get("dependencies")
                .and_then(|d| d.get("follow-redirects"))
                .and_then(|d| d.str_at("version")),
            Some("1.15.6")
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rewrites_replace_the_previous_lock() {
        let dir = temp_dir();
        let mut lock = Lockfile::open(&dir).unwrap();
        lock.write(&roots(), &sample_plan()).unwrap();
        lock.write(&roots(), &Plan::from_items(Vec::new())).unwrap();
        assert!(lock.packages().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
