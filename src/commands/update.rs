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
    run(true)
}

fn run(apply: bool) -> Result<(), String> {
    let ctx = context()?;
    let packages = ctx.db.all()?;
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

    // Move each current zip into prev/ before the new version lands, then
    // let the download pool write the replacements and the db writer move
    // the version fields forward.
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
                })
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
