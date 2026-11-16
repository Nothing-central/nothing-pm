//! Dependency planning: work out every package (and version) a project
//! needs, preferring the local store, and only then go to a registry.
//!
//! Rules that matter here (spec §3, §4, §6, §8):
//!  - the store is consulted before any network call;
//!  - a stored version is reused when it satisfies the requirement;
//!  - `--forceall` only accepts an exact stored version and never writes;
//!  - when two packages want different versions of one dependency we ask,
//!    and "both" keeps the extra version nested under its requester.
//!
//! npm compatibility rules that also live here:
//!  - `optionalDependencies` are planned like normal dependencies but may
//!    fail: no network, no published match, or a binary built for another
//!    platform only produces a warning;
//!  - `os`/`cpu`/`libc` guards on a version are honoured, which is what
//!    turns the twenty `@esbuild/*` optional packages into the one this
//!    machine can actually run;
//!  - `peerDependencies` are installed next to the package that asks for
//!    them (npm 7+ behaviour), except peers marked optional in
//!    `peerDependenciesMeta`.

use crate::cli::Source;
use crate::db::{Db, StoredDep, StoredPackage};
use crate::http::Client;
use crate::registry::{self, Packument, Platform, RegistryError, ResolveError, VersionMeta};
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
    /// Only reachable through `optionalDependencies`.
    pub optional: bool,
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
    /// The only published version is built for a different os/cpu/libc and
    /// the user asked for it by name.
    Platform {
        name: String,
        version: String,
        platform: String,
    },
    Db(String),
    Offline(Vec<String>),
}

impl PlanError {
    pub fn message(&self) -> String {
        match self {
            PlanError::Missing { name, error, .. } => format!("{name}: {}", error.message()),
            PlanError::Registry { name, error } => format!("{name}: {}", error.message()),
            PlanError::Platform {
                name,
                version,
                platform,
            } => format!(
                "{name} {version} does not support this machine ({platform}) — the registry only has binaries for other platforms"
            ),
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
    /// Non-fatal problems: skipped optional packages, missing peers.
    pub warnings: Vec<String>,
}

impl Plan {
    pub fn store_hits(&self) -> usize {
        self.items.iter().filter(|i| i.in_store).count()
    }

    pub fn to_fetch(&self) -> Vec<&PlanItem> {
        self.items
            .iter()
            .filter(|i| !i.in_store && i.tarball.is_some())
            .collect()
    }

    pub fn primary(&self) -> impl Iterator<Item = &PlanItem> {
        self.items.iter().filter(|i| i.nested_under.is_none())
    }

    pub fn find(&self, name: &str) -> Option<&PlanItem> {
        self.primary().find(|i| i.name == name)
    }

    /// Build a plan from a lockfile: nothing to resolve, everything known.
    pub fn from_items(items: Vec<PlanItem>) -> Plan {
        Plan {
            items,
            conflicts: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

pub struct Planner<'a> {
    db: &'a Db,
    client: &'a Client,
    pub forceall: bool,
    pub offline: bool,
    platform: Platform,
}

struct Request {
    name: String,
    range: Option<String>,
    source: Source,
    requester: Option<usize>,
    optional: bool,
}

impl<'a> Planner<'a> {
    pub fn new(db: &'a Db, client: &'a Client, forceall: bool, offline: bool) -> Planner<'a> {
        Planner {
            db,
            client,
            forceall,
            offline,
            platform: Platform::current(),
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
        let mut warnings: Vec<String> = Vec::new();
        let mut cache: HashMap<(String, Source), Packument> = HashMap::new();
        let mut offline_misses: Vec<String> = Vec::new();
        let mut platform_skips: usize = 0;

        let mut queue: VecDeque<Request> = roots
            .iter()
            .map(|r| Request {
                name: r.name.clone(),
                range: r.range.clone(),
                source: r.source,
                requester: None,
                optional: false,
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
                // An optional dependency that disagrees with the tree is not
                // worth a prompt: the chosen version already works for
                // whoever asked for it first.
                let choice = if req.optional {
                    ConflictChoice::Keep
                } else {
                    decide(&conflict)
                };
                if choice == ConflictChoice::Both {
                    let meta = match self.registry_meta(&req, &mut cache) {
                        Ok(m) => m,
                        Err(error) => {
                            if req.optional {
                                warnings.push(optional_skip(&req, &error));
                                continue;
                            }
                            return Err(error);
                        }
                    };
                    if !registry::supports(&meta, &self.platform) {
                        if req.optional {
                            platform_skips += 1;
                            continue;
                        }
                        return Err(platform_error(&req.name, &meta, &self.platform));
                    }
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
                        &meta,
                        req.name.clone(),
                        req.source,
                        false,
                        Some(meta.tarball.clone()),
                        nested_under,
                        req.optional,
                    );
                    enqueue_deps(&mut queue, idx, &items);
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
                    let idx = push_stored(&mut items, &stored, req.optional);
                    chosen.insert(req.name.clone(), idx);
                    enqueue_deps(&mut queue, idx, &items);
                    continue;
                }

                if (self.offline || self.forceall) && !self.forceall {
                    // Offline with a version mismatch: ask (spec §4). An
                    // optional package never interrupts a build to ask.
                    let accept = req.optional
                        || on_mismatch(&stored.name, &stored.version, req.range.as_deref());
                    if accept {
                        let idx = push_stored(&mut items, &stored, req.optional);
                        chosen.insert(req.name.clone(), idx);
                        enqueue_deps(&mut queue, idx, &items);
                        continue;
                    }
                }
            }

            // 2. registry
            if self.offline {
                if req.optional {
                    warnings.push(format!(
                        "skipped optional {} (not in the store and offline)",
                        req.name
                    ));
                } else {
                    offline_misses.push(req.name.clone());
                }
                continue;
            }

            let meta = match self.registry_meta(&req, &mut cache) {
                Ok(m) => m,
                Err(error) => {
                    if req.optional {
                        warnings.push(optional_skip(&req, &error));
                        continue;
                    }
                    return Err(error);
                }
            };
            if !registry::supports(&meta, &self.platform) {
                if req.optional {
                    platform_skips += 1;
                    continue;
                }
                return Err(platform_error(&req.name, &meta, &self.platform));
            }
            let idx = push_item(
                &mut items,
                &meta,
                req.name.clone(),
                req.source,
                false,
                Some(meta.tarball.clone()),
                None,
                req.optional,
            );
            chosen.insert(req.name.clone(), idx);
            enqueue_deps(&mut queue, idx, &items);
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

        if platform_skips > 0 {
            warnings.push(format!(
                "{platform_skips} optional package(s) skipped: built for a different platform ({})",
                self.platform.label()
            ));
        }

        Ok(Plan {
            items,
            conflicts,
            warnings,
        })
    }

    fn registry_meta(
        &self,
        req: &Request,
        cache: &mut HashMap<(String, Source), Packument>,
    ) -> Result<VersionMeta, PlanError> {
        let packument = self.packument(req.source, &req.name, cache)?;
        let meta = registry::resolve(packument, req.range.as_deref()).map_err(|error| {
            PlanError::Missing {
                name: req.name.clone(),
                error,
            }
        })?;
        Ok(meta.clone())
    }

    fn packument<'b>(
        &self,
        source: Source,
        name: &str,
        cache: &'b mut HashMap<(String, Source), Packument>,
    ) -> Result<&'b Packument, PlanError> {
        let key = (name.to_string(), source);
        if !cache.contains_key(&key) {
            let packument = registry::fetch_packument(self.client, source, name).map_err(|error| {
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

fn optional_skip(req: &Request, error: &PlanError) -> String {
    format!("skipped optional {}: {}", req.name, error.message())
}

fn platform_error(name: &str, meta: &VersionMeta, platform: &Platform) -> PlanError {
    PlanError::Platform {
        name: name.to_string(),
        version: meta.version.to_string(),
        platform: platform.label(),
    }
}

fn push_stored(items: &mut Vec<PlanItem>, stored: &StoredPackage, optional: bool) -> usize {
    let idx = items.len();
    items.push(PlanItem {
        name: stored.name.clone(),
        version: stored.version.clone(),
        source: stored.source,
        // Keep the download URL: a lockfile rebuilt from the store must stay
        // installable on a machine with an empty store.
        tarball: stored.tarball.clone(),
        integrity: stored.integrity.clone(),
        beta: stored.beta,
        checksum: stored.checksum.clone(),
        deps: stored.deps.clone(),
        in_store: true,
        nested_under: None,
        optional,
    });
    idx
}

fn enqueue_deps(queue: &mut VecDeque<Request>, idx: usize, items: &[PlanItem]) {
    for dep in items[idx].deps.clone() {
        queue.push_back(Request {
            name: dep.name,
            range: dep.version.or(Some(dep.range)),
            source: dep.source,
            requester: Some(idx),
            optional: dep.optional,
        });
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
    optional: bool,
) -> usize {
    let mut deps: Vec<StoredDep> = meta
        .deps
        .iter()
        .map(|d| stored_dep(d, false))
        .collect();
    // npm 7+: peers are installed alongside the package that declares them.
    for peer in &meta.peer {
        if !deps.iter().any(|d| d.name == peer.name) {
            deps.push(stored_dep(peer, false));
        }
    }
    // Optional entries override a same-named regular dependency.
    deps.retain(|d| !meta.optional.iter().any(|o| o.name == d.name));
    for opt in &meta.optional {
        deps.push(stored_dep(opt, true));
    }

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
        optional,
    });
    idx
}

fn stored_dep(dep: &registry::DepRef, optional: bool) -> StoredDep {
    StoredDep {
        name: dep.name.clone(),
        range: dep.range.clone(),
        version: None,
        source: dep.source,
        checksum: None,
        optional,
    }
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

    fn meta(name: &str) -> VersionMeta {
        VersionMeta {
            version: Version::parse("1.0.0").unwrap(),
            tarball: format!("https://registry/{name}.tgz"),
            integrity: None,
            deps: Vec::new(),
            optional: Vec::new(),
            peer: Vec::new(),
            os: Vec::new(),
            cpu: Vec::new(),
            libc: Vec::new(),
        }
    }

    #[test]
    fn optional_and_peer_edges_reach_the_tree() {
        let mut m = meta("esbuild");
        m.deps.push(registry::DepRef {
            name: "core".into(),
            range: "^1".into(),
            source: Source::Npm,
        });
        m.optional.push(registry::DepRef {
            name: "@esbuild/linux-x64".into(),
            range: "0.25.0".into(),
            source: Source::Npm,
        });
        m.peer.push(registry::DepRef {
            name: "react".into(),
            range: ">=18".into(),
            source: Source::Npm,
        });

        let mut items = Vec::new();
        push_item(
            &mut items,
            &m,
            "esbuild".into(),
            Source::Npm,
            false,
            Some(m.tarball.clone()),
            None,
            false,
        );

        let deps = &items[0].deps;
        let by_name = |n: &str| deps.iter().find(|d| d.name == n).unwrap();
        assert!(!by_name("core").optional);
        assert!(!by_name("react").optional);
        assert!(by_name("@esbuild/linux-x64").optional);
    }
}
