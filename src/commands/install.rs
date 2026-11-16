//! `nth install`, `nth i` and `nth add`.

use super::{context, ensure_packages, install_plan, Context};
use crate::cli::{PackageSpec, Source};
use crate::install as installer;
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

    let (plan, downloaded) = ensure_packages(&ctx, &roots, forceall)?;
    let node_modules = installer::node_modules(&project);

    ui::blank();
    ui::info("arranging folders...");
    let count = install_plan(&ctx, &plan, &node_modules, &downloaded, forceall)?;

    ui::blank();
    ui::ok(&format!("{count} packages installed"));
    if forceall {
        ui::warn("forceall mode: nothing was saved to store");
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

fn stored_source(ctx: &Context, name: &str) -> Source {
    ctx.db
        .get(name)
        .ok()
        .flatten()
        .map(|p| p.source)
        .unwrap_or(Source::Npm)
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
