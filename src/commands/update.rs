//! `nth update database` and `nth check updates`.
//!
//! These are the only commands that look for newer versions — nothing else
//! in nth will ever ping a registry to ask "is there something better".

use super::context;
use crate::cli::Source;
use crate::db::{StoredDep, StoredPackage};
use crate::http::Client;
use crate::parallel::{self, Job};
use crate::registry::{self, VersionMeta};
use crate::semver::Version;
use crate::ui;
use std::collections::VecDeque;
use std::sync::Mutex;

struct Candidate {
    name: String,
    current: String,
    source: Source,
    latest: Option<VersionMeta>,
}

impl Candidate {
    fn latest_version(&self) -> Option<String> {
        self.latest.as_ref().map(|m| m.version.to_string())
    }

    fn is_newer(&self) -> bool {
        let latest = match &self.latest {
            Some(m) => &m.version,
            None => return false,
        };
        match Version::parse(&self.current) {
            Ok(current) => latest > &current,
            Err(_) => true,
        }
    }
}

pub fn run_check() -> Result<(), String> {
    run(false)
}

pub fn run_update() -> Result<(), String> {
    let outcome = run(true);
    // Even when the store had nothing to update (or the registry was
    // unreachable), it is worth saying whether nth itself is current.
    check_self_update();
    outcome
}

/// `nth update` also reports whether nothing package manager itself has a
/// newer release. It never replaces the running binary: the installer is the
/// upgrade path, and overwriting an executable needs root on most systems.
fn check_self_update() {
    const LATEST: &str =
        "https://api.github.com/repos/Nothing-central/nothing-pm/releases/latest";
    let client = Client::new();
    let json = match client.get_json(LATEST) {
        // Offline, rate limited, or no release published yet: stay quiet
        // rather than turning an update run into an error.
        Ok(json) => json,
        Err(_) => return,
    };
    let tag = match json.str_at("tag_name") {
        Some(tag) => tag.trim_start_matches('v'),
        None => return,
    };
    let current = env!("CARGO_PKG_VERSION");
    let latest = match Version::parse(tag) {
        Ok(v) => v,
        Err(_) => return,
    };
    let mine = match Version::parse(current) {
        Ok(v) => v,
        Err(_) => return,
    };

    ui::blank();
    if latest > mine {
        ui::warn(&format!(
            "nothing package manager {tag} is available (you have {current})"
        ));
        ui::info("  upgrade with the installer:");
        ui::info("  curl -fsSL https://raw.githubusercontent.com/Nothing-central/nothing-pm/main/install.sh | bash");
        ui::info("  windows: irm https://raw.githubusercontent.com/Nothing-central/nothing-pm/main/install.ps1 | iex");
    } else {
        ui::info(&format!(
            "nothing package manager {current} is the latest release"
        ));
    }
}

fn run(apply: bool) -> Result<(), String> {
    let ctx = context()?;
    let packages = ctx.db.active_all()?;
    if packages.is_empty() {
        ui::warn("store is empty — nothing to check");
        return Ok(());
    }
    if !crate::http::is_online() {
        ui::warn("it appears you are offline — cannot reach a registry");
        return Err("offline".into());
    }

    ui::info(&format!("checking {} packages against the registry...", packages.len()));
    let client = Client::new();
    let candidates = lookup(&client, &packages, 10);

    let available: Vec<&Candidate> = candidates.iter().filter(|c| c.is_newer()).collect();
    if available.is_empty() {
        ui::blank();
        ui::ok("everything in the store is up to date");
        return Ok(());
    }

    ui::blank();
    for candidate in &available {
        ui::status(
            ui::ARROW,
            &format!("{} {}", candidate.name, candidate.current),
            &format!(
                "{} available",
                candidate.latest_version().unwrap_or_default()
            ),
        );
    }
    ui::blank();

    if !apply {
        ui::info(&format!(
            "{} updates available — run `nth update database` to apply",
            available.len()
        ));
        return Ok(());
    }

    // Each new version gets its own zip; the download writer stores it and
    // moves the db's active marker forward, leaving the old version intact
    // for rollback.
    let mut jobs = Vec::new();
    for candidate in &available {
        let meta = match &candidate.latest {
            Some(m) => m,
            None => continue,
        };
        // The download writer archives the outgoing zip before replacing it.
        jobs.push(Job {
            name: candidate.name.clone(),
            version: meta.version.to_string(),
            source: candidate.source,
            tarball: meta.tarball.clone(),
            integrity: meta.integrity.clone(),
            beta: meta.version.is_prerelease(),
            deps: meta
                .deps
                .iter()
                .map(|d| StoredDep {
                    name: d.name.clone(),
                    range: d.range.clone(),
                    version: None,
                    source: d.source,
                    checksum: None,
                    optional: false,
                })
                .chain(meta.optional.iter().map(|d| StoredDep {
                    name: d.name.clone(),
                    range: d.range.clone(),
                    version: None,
                    source: d.source,
                    checksum: None,
                    optional: true,
                }))
                .chain(meta.peer.iter().map(|d| StoredDep {
                    name: d.name.clone(),
                    range: d.range.clone(),
                    version: None,
                    source: d.source,
                    checksum: None,
                    optional: false,
                }))
                .collect(),
            peers: meta
                .peer
                .iter()
                .map(|p| (p.name.clone(), p.range.clone()))
                .collect(),
        });
    }

    ui::info(&format!("updating {} packages...", jobs.len()));
    ui::blank();
    let events = ui::Events::new();
    let outcome = parallel::run(
        &ctx.store,
        &client,
        jobs,
        parallel::MAX_WORKERS,
        &ctx.store.db_path(),
        true,
        &events,
    );

    ui::blank();
    ui::ok(&format!(
        "{} packages updated",
        outcome.done.len()
    ));
    if !outcome.failed.is_empty() {
        for (name, err) in &outcome.failed {
            ui::miss(&format!("{name}: {err}"));
        }
        return Err(format!("{} updates failed", outcome.failed.len()));
    }
    Ok(())
}

fn lookup(client: &Client, packages: &[StoredPackage], workers: usize) -> Vec<Candidate> {
    let queue = Mutex::new(VecDeque::from(packages.to_vec()));
    let results = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        for _ in 0..workers.max(1) {
            scope.spawn(|| loop {
                let pkg = {
                    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                    q.pop_front()
                };
                let pkg = match pkg {
                    Some(p) => p,
                    None => break,
                };
                let latest = registry::fetch_packument(client, pkg.source, &pkg.name)
                    .ok()
                    .and_then(|packument| {
                        packument.latest().map(|m| m.clone())
                    });
                results
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(Candidate {
                        name: pkg.name.clone(),
                        current: pkg.version.clone(),
                        source: pkg.source,
                        latest,
                    });
            });
        }
    });

    let mut out = results.into_inner().unwrap_or_default();
    out.sort_by(|a: &Candidate, b: &Candidate| a.name.cmp(&b.name));
    out
}
