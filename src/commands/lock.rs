//! `nth lock` — inspect and export `agree.db`, the project lockfile.
//!
//! The lockfile is written by `nth i` and `nth install`; this module owns
//! both the writing helpers the install commands call and the read-only
//! display/export command.

use super::{stored_source, Context};
use crate::http::Client;
use crate::lockfile::Lockfile;
use crate::project::{self, Manifest};
use crate::resolver::{Conflict, ConflictChoice, Plan, Planner, Root};
use crate::ui;
use std::path::Path;

/// `nth lock` / `nth lock json`.
pub fn run(json: bool) -> Result<(), String> {
    let root = project::project_root()?;
    if !Lockfile::exists(&root) {
        ui::warn("no agree.db in this project yet — run `nth i` to create it");
        return Ok(());
    }
    let lock = Lockfile::open(&root)?;
    let packages = lock.packages()?;
    let roots = lock.roots()?;

    if json {
        let path = lock.write_json()?;
        ui::ok(&format!(
            "wrote {} ({} packages)",
            path.display(),
            packages.len()
        ));
        return Ok(());
    }

    ui::info("🔒 agree.db — the project lockfile");
    if packages.is_empty() {
        ui::warn("the lockfile is empty — run `nth i`");
        return Ok(());
    }

    let mut rows = Vec::with_capacity(packages.len());
    for pkg in &packages {
        rows.push(vec![
            pkg.name.clone(),
            pkg.version.clone(),
            pkg.source.as_str().to_string(),
            if pkg.optional { "optional" } else { "" }.to_string(),
            pkg.nested_under.clone().unwrap_or_else(|| "—".into()),
        ]);
    }
    ui::table(
        &["name", "version", "source", "kind", "nested under"],
        &[28, 12, 8, 10, 16],
        &rows,
    );
    ui::info(&format!(
        " {} packages, {} roots    {}",
        packages.len(),
        roots.len(),
        lock.path().display()
    ));
    ui::info(" `nth lock json` writes the same lockfile as agree.json");
    Ok(())
}

/// Write `agree.db` from a plan that was just installed.
pub fn record(project: &Path, roots: &[Root], plan: &Plan) -> Result<(), String> {
    let mut lock = Lockfile::open(project)?;
    let count = lock.write(roots, plan)?;
    ui::info(&format!("🔒 agree.db updated ({count} packages)"));
    Ok(())
}

/// Replan the whole project from the store and rewrite the lock. `nth
/// install <pkg>` only planned the packages it was given, so the lock is
/// rebuilt from package.json once the new dependency is recorded.
pub fn refresh(ctx: &Context, project: &Path) -> Result<(), String> {
    let manifest = Manifest::read(&project.join("package.json"))?;
    if manifest.dependencies.is_empty() {
        return Ok(());
    }
    let roots: Vec<Root> = manifest
        .dependencies
        .iter()
        .map(|(name, range)| Root {
            name: name.clone(),
            range: Some(range.clone()),
            source: stored_source(ctx, name),
        })
        .collect();

    let client = Client::new();
    let planner = Planner::new(&ctx.db, &client, false, !crate::http::is_online());
    let mut decide = |_: &Conflict| ConflictChoice::Keep;
    let mut on_mismatch = |_: &str, _: &str, _: Option<&str>| true;
    match planner.plan(&roots, &mut decide, &mut on_mismatch) {
        Ok(plan) => record(project, &roots, &plan),
        Err(e) => Err(e.message()),
    }
}
