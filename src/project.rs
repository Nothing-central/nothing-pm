//! package.json reading and the small amount of writing `nth install`
//! needs when it records a new dependency.

use crate::json::Json;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Manifest {
    pub path: PathBuf,
    pub name: Option<String>,
    /// dependencies + devDependencies, in file order.
    pub dependencies: Vec<(String, String)>,
    raw: Json,
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Manifest, String> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let raw = Json::parse(&text).map_err(|e| format!("{} is not valid json: {e}", path.display()))?;
        let mut dependencies = Vec::new();
        for section in ["dependencies", "devDependencies"] {
            if let Some(fields) = raw.get(section).and_then(|s| s.as_obj()) {
                for (name, range) in fields {
                    if let Some(range) = range.as_str() {
                        dependencies.push((name.clone(), range.to_string()));
                    }
                }
            }
        }
        Ok(Manifest {
            path: path.to_path_buf(),
            name: raw.str_at("name").map(|s| s.to_string()),
            dependencies,
            raw,
        })
    }

    /// Record a dependency without disturbing anything else in the file.
    pub fn add_dependency(&mut self, name: &str, range: &str) -> Result<(), String> {
        let mut fields = match self.raw.as_obj() {
            Some(f) => f.clone(),
            None => return Err("package.json is not an object".into()),
        };
        let mut deps = match self
            .raw
            .get("dependencies")
            .and_then(|d| d.as_obj())
            .cloned()
        {
            Some(d) => d,
            None => Vec::new(),
        };

        let existing = deps.iter_mut().find(|(k, _)| k == name);
        match existing {
            Some(slot) => slot.1 = Json::s(range),
            None => deps.push((name.to_string(), Json::s(range))),
        }

        let new_deps = Json::Obj(deps);
        match fields.iter_mut().find(|(k, _)| k == "dependencies") {
            Some(slot) => slot.1 = new_deps,
            None => {
                // Keep dependencies near the top, after name/version.
                let insert_at = fields
                    .iter()
                    .position(|(k, _)| k == "devDependencies")
                    .unwrap_or(fields.len());
                fields.insert(insert_at, ("dependencies".to_string(), new_deps));
            }
        }
        self.raw = Json::Obj(fields);
        fs::write(&self.path, self.raw.to_pretty())
            .map_err(|e| format!("cannot write {}: {e}", self.path.display()))?;
        Ok(())
    }
}

/// Walk up from `start` looking for package.json, like npm does.
pub fn find_manifest(start: &Path) -> Result<PathBuf, String> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("package.json");
        if candidate.is_file() {
            return Ok(candidate);
        }
        if !dir.pop() {
            return Err(format!(
                "no package.json found in {} or any parent directory",
                start.display()
            ));
        }
    }
}

pub fn project_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot read current directory: {e}"))?;
    let manifest = find_manifest(&cwd)?;
    Ok(manifest.parent().map(|p| p.to_path_buf()).unwrap_or(cwd))
}
