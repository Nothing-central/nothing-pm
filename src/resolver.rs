//! Dependency planning: work out every package (and version) a project
//! needs, preferring the local store, and only then go to a registry.
//!
//! Rules that matter here (spec §3, §4, §6, §8):
//!  - the store is consulted before any network call;
//!  - a stored version is reused when it satisfies the requirement;
//!  - `--forceall` only accepts an exact stored version and never writes;
//!  - when two packages want different versions of one dependency we ask,
//!    and "both" keeps the extra version nested under its requester.

use crate::cli::Source;
use crate::db::{Db, StoredDep};
use crate::http::Client;
use crate::registry::{self, Packument, RegistryError, ResolveError};
use crate::semver::{Range, Version};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug)]
pub struct Root {
    pub name: String,
    pub range: Option<String>,
    pub source: Source,
}

#[derive(Clone, Debug)]
pub struct PlanItem {
    pub name: String,
    pub version: String,
    pub source: Source,
    pub tarball: Option<String>,
    pub integrity: Option<String>,
    pub beta: bool,
    pub checksum: Option<String>,
    pub deps: Vec<StoredDep>,
    pub in_store: bool,
    /// Set for the second copy of a conflicted dependency, placed under its
    /// requester's node_modules.
    pub nested_under: Option<String>,
}

impl PlanItem {
    pub fn label(&self) -> String {
        format!("{} {}", self.name, self.version)
    }
}

#[derive(Clone, Debug)]
pub struct Conflict {
    pub name: String,
    pub chosen: String,
    pub wanted_range: String,
    pub required_by: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Use the version that was already chosen.
    Keep,
    /// Download the other version too and nest it under the requester.
    Both,
}

#[derive(Debug)]
pub enum PlanError {
    Missing {
        name: String,
        error: ResolveError,
    },
    Registry {
        name: String,
        error: RegistryError,
    },
    Db(String),
    Offline(Vec<String>),
}

impl PlanError {
    pub fn message(&self) -> String {
        match self {
            PlanError::Missing { name, error, .. } => format!("{name}: {}", error.message()),
            PlanError::Registry { name, error } => format!("{name}: {}", error.message()),
            PlanError::Db(m) => m.clone(),
            PlanError::Offline(names) => format!(
                "not in the store and no internet connection: {}",
                names.join(", ")
            ),
        }
    }
}

pub struct Plan {
    pub items: Vec<PlanItem>,
    pub conflicts: Vec<Conflict>,
}

impl Plan {
    pub fn store_hits(&self) -> usize {
        self.items.iter().filter(|i| i.in_store).count()
    }

    pub fn to_fetch(&self) -> Vec<&PlanItem> {
        self.items.iter().filter(|i| !i.in_store && i.tarball.is_some()).collect()
    }

    pub fn primary(&self) -> impl Iterator<Item = &PlanItem> {
        self.items.iter().filter(|i| i.nested_under.is_none())
    }

    pub fn find(&self, name: &str) -> Option<&PlanItem> {
        self.primary().find(|i| i.name == name)
    }
}

pub struct Planner<'a> {
    db: &'a Db,
    client: &'a Client,
    pub forceall: bool,
    pub offline: bool,
}

struct Request {
    name: String,
    range: Option<String>,
    source: Source,
    requester: Option<usize>,
}

impl<'a> Planner<'a> {
    pub fn new(db: &'a Db, client: &'a Client, forceall: bool, offline: bool) -> Planner<'a> {
        Planner {
            db,
            client,
            forceall,
            offline,
        }
    }

    /// `decide` is only called for genuinely conflicting requirements.
    /// `on_mismatch` is called when the store has the package but not a
    /// satisfying version and we cannot reach a registry; returning true
    /// accepts the stored version anyway.
    pub fn plan(
        &self,
        roots: &[Root],
        decide: &mut dyn FnMut(&Conflict) -> ConflictChoice,
        on_mismatch: &mut dyn FnMut(&str, &str, Option<&str>) -> bool,
    ) -> Result<Plan, PlanError> {
        let mut items: Vec<PlanItem> = Vec::new();
        let mut chosen: HashMap<String, usize> = HashMap::new();
        let mut conflicts: Vec<Conflict> = Vec::new();
        let mut cache: HashMap<(String, Source), Packument> = HashMap::new();
        let mut offline_misses: Vec<String> = Vec::new();

        let mut queue: VecDeque<Request> = roots
            .iter()
            .map(|r| Request {
                name: r.name.clone(),
                range: r.range.clone(),
                source: r.source,
                requester: None,
            })
            .collect();

        while let Some(req) = queue.pop_front() {
            if let Some(&idx) = chosen.get(&req.name) {
                let existing = items[idx].version.clone();
                if satisfies(&existing, req.range.as_deref()) {
                    continue;
                }
                let conflict = Conflict {
                    name: req.name.clone(),
                    chosen: existing,
                    wanted_range: req.range.clone().unwrap_or_else(|| "*".into()),
                    required_by: req
                        .requester
                        .map(|i| items[i].name.clone())
                        .unwrap_or_else(|| "this project".into()),
                };
                if decide(&conflict) == ConflictChoice::Both {
                    let packument = self.packument(req.source, &req.name, &mut cache)?;
                    let meta = registry::resolve(packument, req.range.as_deref()).map_err(|error| {
                        PlanError::Missing {
                            name: req.name.clone(),
                            error,
                        }
                    })?;
                    let version = meta.version.to_string();
                    if items
                        .iter()
                        .any(|i| i.name == req.name && i.version == version)
                    {
                        continue;
                    }
                    let nested_under = req.requester.map(|i| items[i].name.clone());
                    let idx = push_item(
                        &mut items,
                        meta,
                        req.name.clone(),
                        req.source,
                        false,
                        Some(meta.tarball.clone()),
                        nested_under,
                    );
                    let deps: Vec<(String, String, Source)> = items[idx]
                        .deps
                        .iter()
                        .map(|d| (d.name.clone(), d.range.clone(), d.source))
                        .collect();
                    for (name, range, source) in deps {
                        queue.push_back(Request {
                            name,
                            range: Some(range),
                            source,
                            requester: Some(idx),
                        });
                    }
                } else {
                    conflicts.push(conflict);
                }
                continue;
            }

            // 1. local store first
            if let Some(stored) = self.db.get(&req.name).map_err(PlanError::Db)? {
                let exact_ok = req
                    .range
                    .as_deref()
                    .map(|r| r.trim() == stored.version)
                    .unwrap_or(false);
                let usable = if self.forceall {
                    exact_ok
                } else {
                    satisfies(&stored.version, req.range.as_deref())
                };

                if usable {
                    let idx = items.len();
                    let deps = stored.deps.clone();
                    items.push(PlanItem {
                        name: stored.name.clone(),
                        version: stored.version.clone(),
                        source: stored.source,
                        tarball: None,
                        integrity: None,
                        beta: stored.beta,
                        checksum: stored.checksum.clone(),
                        deps,
                        in_store: true,
                        nested_under: None,
                    });
                    chosen.insert(req.name.clone(), idx);
                    for dep in items[idx].deps.clone() {
                        queue.push_back(Request {
                            name: dep.name,
                            range: dep.version.or(Some(dep.range)),
                            source: dep.source,
                            requester: Some(idx),
                        });
                    }
                    continue;
                }

                if (self.offline || self.forceall) && !self.forceall {
                    // Offline with a version mismatch: ask (spec §4).
                    if on_mismatch(&stored.name, &stored.version, req.range.as_deref()) {
                        let idx = items.len();
                        let deps = stored.deps.clone();
                        items.push(PlanItem {
                            name: stored.name.clone(),
                            version: stored.version.clone(),
                            source: stored.source,
                            tarball: None,
                            integrity: None,
                            beta: stored.beta,
                            checksum: stored.checksum.clone(),
                            deps,
                            in_store: true,
                            nested_under: None,
                        });
                        chosen.insert(req.name.clone(), idx);
                        for dep in items[idx].deps.clone() {
                            queue.push_back(Request {
                                name: dep.name,
                                range: dep.version.or(Some(dep.range)),
                                source: dep.source,
                                requester: Some(idx),
                            });
                        }
                        continue;
                    }
                }
            }

            // 2. registry
            if self.offline {
                offline_misses.push(req.name.clone());
                continue;
            }

            let packument = self.packument(req.source, &req.name, &mut cache)?;
            let meta = registry::resolve(packument, req.range.as_deref()).map_err(|error| {
                PlanError::Missing {
                    name: req.name.clone(),
                    error,
                }
            })?;
            let idx = push_item(
                &mut items,
                meta,
                req.name.clone(),
                req.source,
                false,
                Some(meta.tarball.clone()),
                None,
            );
            chosen.insert(req.name.clone(), idx);
            let deps: Vec<(String, String, Source)> = items[idx]
                .deps
                .iter()
                .map(|d| (d.name.clone(), d.range.clone(), d.source))
                .collect();
            for (name, range, source) in deps {
                queue.push_back(Request {
                    name,
                    range: Some(range),
                    source,
                    requester: Some(idx),
                });
            }
        }

        if !offline_misses.is_empty() {
            return Err(PlanError::Offline(offline_misses));
        }

        // Fill in resolved versions for every dependency edge so the db (and
        // therefore `nth info`) records a complete tree. Snapshot first: the
        // edge list lives inside the very items we are rewriting.
        let resolved: HashMap<String, (String, Option<String>)> = chosen
            .iter()
            .map(|(name, idx)| {
                (
                    name.clone(),
                    (items[*idx].version.clone(), items[*idx].checksum.clone()),
                )
            })
            .collect();
        for item in items.iter_mut() {
            for dep in item.deps.iter_mut() {
                if let Some((version, checksum)) = resolved.get(&dep.name) {
                    if dep.version.is_none() {
                        dep.version = Some(version.clone());
                    }
                    if dep.checksum.is_none() {
                        dep.checksum = checksum.clone();
                    }
                }
            }
        }

        Ok(Plan { items, conflicts })
    }

    fn packument<'b>(
        &self,
        source: Source,
        name: &str,
        cache: &'b mut HashMap<(String, Source), Packument>,
    ) -> Result<&'b Packument, PlanError> {
        let key = (name.to_string(), source);
        if !cache.contains_key(&key) {
            let packument =
                registry::fetch_packument(self.client, source, name).map_err(|error| {
                    PlanError::Registry {
                        name: name.to_string(),
                        error,
                    }
                })?;
            cache.insert(key.clone(), packument);
        }
        Ok(cache.get(&key).expect("just inserted"))
    }
}

fn push_item(
    items: &mut Vec<PlanItem>,
    meta: &registry::VersionMeta,
    name: String,
    source: Source,
    in_store: bool,
    tarball: Option<String>,
    nested_under: Option<String>,
) -> usize {
    let deps = meta
        .deps
        .iter()
        .map(|d| StoredDep {
            name: d.name.clone(),
            range: d.range.clone(),
            version: None,
            source: d.source,
            checksum: None,
        })
        .collect();
    let idx = items.len();
    items.push(PlanItem {
        name,
        version: meta.version.to_string(),
        source,
        tarball,
        integrity: meta.integrity.clone(),
        beta: meta.version.is_prerelease(),
        checksum: None,
        deps,
        in_store,
        nested_under: nested_under.filter(|s| !s.is_empty()),
    });
    idx
}

/// True when `version` satisfies `requirement`. `None`/`*`/`latest` accept
/// whatever the store already holds — the internet is opt-in.
pub fn satisfies(version: &str, requirement: Option<&str>) -> bool {
    let requirement = match requirement {
        None => return true,
        Some(r) => r.trim(),
    };
    if requirement.is_empty() || requirement == "*" || requirement == "latest" {
        return true;
    }
    let parsed = match Version::parse(version) {
        Ok(v) => v,
        Err(_) => return false,
    };
    // A tag comparison ("beta") can only be answered by the registry.
    let range = match Range::parse(requirement) {
        Ok(r) => r,
        Err(_) => return false,
    };
    range.test(&parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn satisfaction_rules() {
        assert!(satisfies("2.2.3", Some("^2.2.0")));
        assert!(!satisfies("2.2.3", Some("^3.0.0")));
        assert!(satisfies("1.0.0", None));
        assert!(satisfies("1.0.0", Some("latest")));
        assert!(satisfies("1.0.0", Some("1.0.0")));
    }
}
