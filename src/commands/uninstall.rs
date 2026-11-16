//! `nth uninstall <pkg>` — remove packages from the current project.
//!
//! This is a project operation: the package directory goes, and the entry
//! leaves package.json. The copy in the global store stays, because that is
//! what makes reinstalling cheap and offline work — `nth delete <pkg>` is
//! the command that removes something from the store.

use crate::install as installer;
use crate::project::{self, Manifest};
use crate::ui;
use std::fs;

pub fn run(names: Vec<String>) -> Result<(), String> {
    let root = project::project_root()?;
    let manifest_path = root.join("package.json");
    let mut manifest = Manifest::read(&manifest_path)?;
    let node_modules = installer::node_modules(&root);

    let mut removed = 0usize;
    for name in &names {
        let dir = installer::package_dir(&node_modules, name);
        let had_files = if dir.exists() {
            fs::remove_dir_all(&dir)
                .map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
            remove_empty_scope(&node_modules, &dir);
            true
        } else {
            false
        };

        let had_entry = manifest.remove_dependency(name)?;

        if had_files || had_entry {
            let mut detail = Vec::new();
            if had_files {
                detail.push("node_modules");
            }
            if had_entry {
                detail.push("package.json");
            }
            ui::ok(&format!("uninstalled {name} ({})", detail.join(" + ")));
            removed += 1;
        } else {
            ui::warn(&format!("{name} is not installed in this project"));
        }
    }

    if removed > 0 {
        ui::blank();
        ui::info("still in the global store — `nth delete <pkg>` removes it from there");
    }
    Ok(())
}

/// `node_modules/@scope` should not linger once its last package is gone.
fn remove_empty_scope(node_modules: &std::path::Path, package_dir: &std::path::Path) {
    let parent = match package_dir.parent() {
        Some(p) => p,
        None => return,
    };
    if parent == node_modules {
        return;
    }
    let empty = fs::read_dir(parent)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false);
    if empty {
        let _ = fs::remove_dir(parent);
    }
}
