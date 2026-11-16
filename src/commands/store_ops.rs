//! `nth view db`, `nth delete` and the three `nth clear` modes.

use super::context;
use crate::cli::{validate_name, ClearPart};
use crate::ui;

pub fn run_view() -> Result<(), String> {
    let ctx = context()?;
    let packages = ctx.db.all()?;

    ui::info("📦 nothing package store");
    if packages.is_empty() {
        ui::warn("store is empty — try `nth add axios` or `nth install axios`");
        return Ok(());
    }

    let mut rows = Vec::new();
    let mut total = 0u64;
    for pkg in &packages {
        total += pkg.size_kb;
        rows.push(vec![
            pkg.name.clone(),
            pkg.version.clone(),
            pkg.prev_version.clone().unwrap_or_else(|| "—".into()),
            ui::size(pkg.size_kb),
            pkg.source.as_str().to_string(),
        ]);
    }
    ui::table(
        &["name", "version", "prev", "size", "source"],
        &[26, 12, 12, 9, 8],
        &rows,
    );
    ui::info(&format!(
        " {} packages    {} total",
        packages.len(),
        ui::size(total)
    ));
    Ok(())
}

pub fn run_delete(name: &str) -> Result<(), String> {
    validate_name(name)?;
    let ctx = context()?;

    let mut removed: Vec<&str> = Vec::new();
    if crate::store::remove_current(&ctx.store, name).map_err(|e| e.to_string())? {
        removed.push("current zip");
    }
    if crate::store::remove_prev(&ctx.store, name).map_err(|e| e.to_string())? {
        removed.push("previous zip");
    }
    if ctx.db.delete(name)? {
        removed.push("database entry");
    }

    if removed.is_empty() {
        ui::warn(&format!("{name} is not in the store"));
    } else {
        ui::ok(&format!("removed {name} ({})", removed.join(", ")));
    }
    Ok(())
}

pub fn run_clear(name: &str, part: ClearPart) -> Result<(), String> {
    validate_name(name)?;
    let ctx = context()?;

    match part {
        ClearPart::All => run_delete(name),
        ClearPart::Prev => {
            let file = crate::store::remove_prev(&ctx.store, name).map_err(|e| e.to_string())?;
            let row = ctx.db.clear_prev(name)?;
            if file || row {
                ui::ok(&format!("cleared the previous version of {name}"));
            } else {
                ui::warn(&format!("no previous version stored for {name}"));
            }
            Ok(())
        }
        ClearPart::New => {
            let stored = ctx.db.get(name)?;
            let has_prev = stored
                .as_ref()
                .map(|s| s.prev_version.is_some())
                .unwrap_or(false);

            if stored.is_none() {
                ui::warn(&format!("{name} is not in the store"));
                return Ok(());
            }
            if !has_prev {
                ui::warn(&format!("no previous version of {name} to roll back to"));
                if ui::confirm(&format!("delete {name} from the store completely?")) {
                    return run_delete(name);
                }
                ui::info(&format!("left {name} untouched"));
                return Ok(());
            }

            crate::store::promote_prev(&ctx.store, name).map_err(|e| e.to_string())?;
            let promoted = ctx
                .db
                .promote_prev(name)?
                .unwrap_or_else(|| "previous version".into());
            ui::ok(&format!("rolled {name} back to {promoted}"));
            Ok(())
        }
    }
}
