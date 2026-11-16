//! `nth info` — write nothing-info.json from the db alone.
//!
//! No network requests, no node_modules scanning: every field here already
//! lives in nothingda.db.

use super::context;
use crate::db::StoredPackage;
use crate::json::Json;
use crate::project::{self, Manifest};
use crate::ui;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run() -> Result<(), String> {
    let ctx = context()?;
    let root = project::project_root()?;
    let manifest = Manifest::read(&root.join("package.json"))?;
    let stored = ctx.db.active_all()?;

    let index: HashMap<&str, &StoredPackage> =
        stored.iter().map(|p| (p.name.as_str(), p)).collect();

    let mut packages = Vec::new();
    let mut visited: Vec<String> = Vec::new();
    for (name, _range) in &manifest.dependencies {
        if let Some(pkg) = index.get(name.as_str()) {
            packages.push(package_json(pkg, &index, 0, &mut visited));
        } else {
            ui::warn(&format!("{name} is not in the store — skipping"));
        }
    }

    let doc = Json::obj(vec![
        (
            "project",
            Json::s(manifest.name.as_deref().unwrap_or("unknown")),
        ),
        ("generated", Json::s(&now_iso8601())),
        ("packages", Json::Arr(packages)),
    ]);

    let path = root.join("nothing-info.json");
    std::fs::write(&path, doc.to_pretty())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    ui::ok(&format!("wrote {}", path.display()));
    Ok(())
}

fn package_json(
    pkg: &StoredPackage,
    index: &HashMap<&str, &StoredPackage>,
    depth: usize,
    visited: &mut Vec<String>,
) -> Json {
    if !visited.contains(&pkg.name) {
        visited.push(pkg.name.clone());
    }

    let dependencies: Vec<Json> = if depth >= 8 {
        Vec::new()
    } else {
        pkg.deps
            .iter()
            .map(|dep| {
                let resolved = index.get(dep.name.as_str());
                let version = dep
                    .version
                    .clone()
                    .or_else(|| resolved.map(|p| p.version.clone()))
                    .unwrap_or_default();
                let checksum = dep
                    .checksum
                    .clone()
                    .or_else(|| resolved.and_then(|p| p.checksum.clone()));

                let mut fields = vec![
                    ("name", Json::s(&dep.name)),
                    ("version", Json::s(&version)),
                    (
                        "checksum",
                        checksum.map(|c| Json::s(&c)).unwrap_or(Json::Null),
                    ),
                ];

                // Recurse into nested dependencies, once per package name.
                if let Some(child) = resolved {
                    if !visited.contains(&child.name) {
                        let nested = package_json(child, index, depth + 1, visited);
                        if let Some(deps) = nested.get("dependencies") {
                            fields.push(("dependencies", deps.clone()));
                        }
                    }
                }
                Json::obj(fields)
            })
            .collect()
    };

    Json::obj(vec![
        ("name", Json::s(&pkg.name)),
        ("version", Json::s(&pkg.version)),
        ("source", Json::s(pkg.source.as_str())),
        (
            "checksum",
            pkg.checksum.clone().map(|c| Json::s(&c)).unwrap_or(Json::Null),
        ),
        ("dependencies", Json::Arr(dependencies)),
    ])
}

fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days → civil date algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}
