//! `nth view db`, `nth delete` and the three `nth clear` modes.
//!
//! The store holds every version it has ever downloaded, so these commands
//! are version-aware: `view` lists every zip, `clear <pkg> prev` drops the
//! rollback version only, and `delete` is the one that removes a package
//! entirely.

use super::context;
use crate::cli::{validate_name, ClearPart};
use crate::ui;

pub fn run_view() -> Result<(), String> {
    let ctx = context()?;
    let packages = ctx.db.all()?;

    ui::info("📦 nothing package manager store");
    if packages.is_empty() {
        ui::warn("store is empty — try `nth add axios` or `nth install axios`");
        return Ok(());
    }

    let mut rows = Vec::new();
    let mut total = 0u64;
    let mut names: Vec<&str> = Vec::new();
    for pkg in &packages {
        total += pkg.size_kb;
        if !names.contains(&pkg.name.as_str()) {
            names.push(&pkg.name);
        }
        rows.push(vec![
            pkg.name.clone(),
            pkg.version.clone(),
            if pkg.active { "active" } else { "kept" }.to_string(),
            pkg.prev_version.clone().unwrap_or_else(|| "—".into()),
            ui::size(pkg.size_kb),
            pkg.source.as_str().to_string(),
        ]);
    }
    ui::table(
        &["name", "version", "state", "prev", "size", "source"],
        &[26, 12, 8, 12, 9, 8],
        &rows,
    );
    ui::info(&format!(
        " {} versions of {} packages    {} total",
        packages.len(),
        names.len(),
        ui::size(total)
    ));
    Ok(())
}

pub fn run_delete(name: &str) -> Result<(), String> {
    validate_name(name)?;
    let ctx = context()?;

    let versions = ctx.db.delete_all(name)?;
    let removed_files =
        crate::store::remove_package(&ctx.store, name).map_err(|e| e.to_string())?;

    if versions.is_empty() && !removed_files {
        ui::warn(&format!("{name} is not in the store"));
    } else {
        ui::ok(&format!(
            "removed {name} ({} version{})",
            versions.len(),
            if versions.len() == 1 { "" } else { "s" }
        ));
    }
    Ok(())
}

pub fn run_clear(name: &str, part: ClearPart) -> Result<(), String> {
    validate_name(name)?;
    let ctx = context()?;

    match part {
        ClearPart::All => run_delete(name),
        ClearPart::Prev => {
            let cleared = ctx.db.clear_prev(name)?;
            match cleared {
                Some(version) => {
                    crate::store::remove_version(&ctx.store, name, &version)
                        .map_err(|e| e.to_string())?;
                    ui::ok(&format!("cleared the previous version of {name} ({version})"));
                }
                None => ui::warn(&format!("no previous version stored for {name}")),
            }
            Ok(())
        }
        ClearPart::New => {
            let stored = ctx.db.get(name)?;
            let active = match stored {
                Some(p) => p,
                None => {
                    ui::warn(&format!("{name} is not in the store"));
                    return Ok(());
                }
            };
            if active.prev_version.is_none() {
                ui::warn(&format!("no previous version of {name} to roll back to"));
                if ui::confirm(&format!("delete every version of {name} from the store?")) {
                    return run_delete(name);
                }
                ui::info(&format!("left {name} untouched"));
                return Ok(());
            }

            // Every version keeps its own zip, so rolling back only moves
            // the active marker back to a file that is still on disk.
            let promoted = ctx
                .db
                .promote_prev(name)?
                .unwrap_or_else(|| "previous version".into());
            ui::ok(&format!("rolled {name} back to {promoted}"));
            Ok(())
        }
    }
}
