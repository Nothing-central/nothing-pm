//! `nth install`, `nth i` and `nth add`.

use super::{context, ensure_packages, fetch_plan, install_plan, lock, stored_source, Context};
use crate::cli::PackageSpec;
use crate::http::Client;
use crate::install as installer;
use crate::lockfile::Lockfile;
use crate::registry;
use crate::project::{self, Manifest};
use crate::resolver::{Plan, Root};
use crate::ui;
use std::path::Path;

pub fn run_install(specs: Vec<PackageSpec>) -> Result<(), String> {
    let ctx = context()?;
    let project = project::project_root()?;
    let roots: Vec<Root> = specs
        .iter()
        .map(|s| Root {
            name: s.name.clone(),
            range: s.range.clone(),
            source: s.source,
        })
        .collect();

    let (plan, downloaded) = ensure_packages(&ctx, &roots, false)?;
    let node_modules = installer::node_modules(&project);

    ui::blank();
    ui::info("arranging folders...");
    let count = install_plan(&ctx, &plan, &node_modules, &downloaded, false)?;
    record_dependencies(&project, &plan, &specs)?;
    refresh_lock(&ctx, &project);

    ui::blank();
    ui::ok(&format!("{count} packages installed"));
    Ok(())
}

pub fn run_install_all(forceall: bool) -> Result<(), String> {
    let ctx = context()?;
    let project = project::project_root()?;
    let manifest = Manifest::read(&project.join("package.json"))?;

    if manifest.dependencies.is_empty() {
        ui::warn("package.json lists no dependencies");
        return Ok(());
    }
    ui::info(&format!(
        "scanning package.json... found {} dependencies",
        manifest.dependencies.len()
    ));
    ui::blank();

    let roots: Vec<Root> = manifest
        .dependencies
        .iter()
        .map(|(name, range)| Root {
            name: name.clone(),
            range: Some(range.clone()),
            source: stored_source(&ctx, name),
        })
        .collect();

    // A lockfile that still matches package.json wins: it is the exact tree
    // this project agreed on, so nothing has to guess or ask a registry.
    let (plan, downloaded) = match locked_plan(&ctx, &project, &roots)? {
        Some(plan) => {
            ui::info("agree.db matches package.json — installing the locked versions");
            let plan = fill_locked_urls(plan)?;
            let downloaded = fetch_plan(&ctx, &plan, false)?;
            (plan, downloaded)
        }
        None => ensure_packages(&ctx, &roots, forceall)?,
    };

    let node_modules = installer::node_modules(&project);

    ui::blank();
    ui::info("arranging folders...");
    let count = install_plan(&ctx, &plan, &node_modules, &downloaded, forceall)?;

    ui::blank();
    ui::ok(&format!("{count} packages installed"));
    if forceall {
        ui::warn("forceall mode: nothing was saved to store");
    } else if let Err(e) = lock::record(&project, &roots, &plan) {
        ui::warn(&format!("could not update agree.db: {e}"));
    }
    Ok(())
}

/// `nth add`: stock the store, touch no project.
pub fn run_add(specs: Vec<PackageSpec>) -> Result<(), String> {
    let ctx = context()?;
    let roots: Vec<Root> = specs
        .iter()
        .map(|s| Root {
            name: s.name.clone(),
            range: s.range.clone(),
            source: s.source,
        })
        .collect();

    let (plan, downloaded) = ensure_packages(&ctx, &roots, false)?;
    ui::blank();
    if downloaded.is_empty() {
        ui::ok(&format!(
            "{} packages already in store",
            plan.store_hits()
        ));
    } else {
        ui::ok(&format!(
            "{} added to store ({} already there)",
            downloaded.len(),
            plan.store_hits()
        ));
    }
    Ok(())
}

/// The lockfile only applies when its roots are the project's current ones.
fn locked_plan(ctx: &Context, project: &Path, roots: &[Root]) -> Result<Option<Plan>, String> {
    if !Lockfile::exists(project) {
        return Ok(None);
    }
    let lockfile = Lockfile::open(project)?;
    if !lockfile.matches(roots)? {
        ui::info("agree.db is stale (package.json changed) — resolving again");
        return Ok(None);
    }
    lockfile.to_plan(&ctx.db)
}

/// The store is asked first (that is what `to_plan` does). Anything the
/// store does not have is fetched as the *exact pinned version* from the
/// registry the lock recorded — npm or jsr — never by re-resolving ranges.
fn fill_locked_urls(mut plan: Plan) -> Result<Plan, String> {
    if plan
        .items
        .iter()
        .all(|item| item.in_store || item.tarball.is_some())
    {
        return Ok(plan);
    }

    let client = Client::new();
    for item in plan.items.iter_mut() {
        if item.in_store || item.tarball.is_some() {
            continue;
        }
        ui::info(&format!(
            "{} {} is locked but not in the store, fetching that exact version from {}",
            item.name,
            item.version,
            item.source.as_str()
        ));
        let packument = registry::fetch_packument(&client, item.source, &item.name).map_err(|e| {
            format!(
                "agree.db pins {} {} but the {} registry could not be read: {}",
                item.name,
                item.version,
                item.source.as_str(),
                e.message()
            )
        })?;
        let meta = registry::resolve(&packument, Some(&item.version)).map_err(|e| {
            format!(
                "agree.db pins {} {}, which no longer exists: {}",
                item.name,
                item.version,
                e.message()
            )
        })?;
        item.tarball = Some(meta.tarball.clone());
        if item.integrity.is_none() {
            item.integrity = meta.integrity.clone();
        }
    }
    Ok(plan)
}

fn refresh_lock(ctx: &Context, project: &Path) {
    if let Err(e) = lock::refresh(ctx, project) {
        ui::warn(&format!("could not update agree.db: {e}"));
    }
}

/// `nth install axios` writes the dependency back into package.json, the
/// file that is the single source of truth for versions.
fn record_dependencies(
    project: &Path,
    plan: &Plan,
    specs: &[PackageSpec],
) -> Result<(), String> {
    let manifest_path = project.join("package.json");
    if !manifest_path.is_file() {
        return Ok(());
    }
    let mut manifest = Manifest::read(&manifest_path)?;
    for spec in specs {
        if let Some(item) = plan.find(&spec.name) {
            let range = spec
                .range
                .clone()
                .unwrap_or_else(|| format!("^{}", item.version));
            manifest.add_dependency(&spec.name, &range)?;
        }
    }
    Ok(())
}
