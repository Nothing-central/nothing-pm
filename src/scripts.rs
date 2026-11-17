//! Lifecycle scripts and `node_modules/.bin`.
//!
//! npm compatibility surface. After a package lands in node_modules:
//!
//!  - its `bin` field becomes a link in the owning `node_modules/.bin`, so
//!    `npx next` and `npm run` can find it;
//!  - its `preinstall`, `install`, `postinstall` and `prepare` scripts run
//!    with that `.bin` on PATH, like npm runs them.
//!
//! `--ignore-scripts` skips running code; the links are pure filesystem
//! state and are always rebuilt, because a stale `.bin` breaks `npx`.

use crate::json::Json;
use crate::ui;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LIFECYCLE: [&str; 4] = ["preinstall", "install", "postinstall", "prepare"];

/// A package as it sits on disk after installation.
#[derive(Clone, Debug)]
pub struct Installed {
    pub name: String,
    /// Where the package code lives, e.g. `node_modules/axios`.
    pub dir: PathBuf,
    /// The node_modules folder this package belongs to; its `.bin` is the
    /// one this package's scripts get on PATH.
    pub node_modules: PathBuf,
}

impl Installed {
    fn manifest(&self) -> Option<Json> {
        let text = fs::read_to_string(self.dir.join("package.json")).ok()?;
        Json::parse(&text).ok()
    }
}

/// Rebuild `node_modules/.bin` for every package just installed.
pub fn link_bins(installed: &[Installed]) -> Result<usize, String> {
    let mut dirs: Vec<&Path> = Vec::new();
    for pkg in installed {
        if !dirs.contains(&pkg.node_modules.as_path()) {
            dirs.push(&pkg.node_modules);
        }
    }
    for dir in &dirs {
        let bin = dir.join(".bin");
        if bin.exists() {
            fs::remove_dir_all(&bin)
                .map_err(|e| format!("cannot clear {}: {e}", bin.display()))?;
        }
    }

    let mut linked = 0usize;
    for pkg in installed {
        let manifest = match pkg.manifest() {
            Some(m) => m,
            None => continue,
        };
        let package_rel = pkg
            .dir
            .strip_prefix(&pkg.node_modules)
            .unwrap_or(&pkg.dir)
            .to_path_buf();
        let bin_dir = pkg.node_modules.join(".bin");
        for (name, relative) in bins(&manifest, &pkg.name) {
            let target = pkg.dir.join(&relative);
            if !target.is_file() {
                continue;
            }
            fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
            let target_rel = Path::new("..").join(&package_rel).join(&relative);
            link(&bin_dir, &name, &target_rel)?;
            linked += 1;
        }
    }
    Ok(linked)
}

/// `{"bin": "cli.js"}`, `{"bin": {"next": "dist/bin/next"}}` or nothing.
fn bins(manifest: &Json, package_name: &str) -> Vec<(String, String)> {
    match manifest.get("bin") {
        Some(Json::Str(path)) => vec![(leaf(package_name), path.clone())],
        Some(Json::Obj(entries)) => entries
            .iter()
            .filter_map(|(name, value)| value.as_str().map(|p| (name.clone(), p.to_string())))
            .collect(),
        _ => Vec::new(),
    }
}

fn leaf(name: &str) -> String {
    match name.rsplit_once('/') {
        Some((_, pkg)) => pkg.to_string(),
        None => name.to_string(),
    }
}

#[cfg(unix)]
fn link(bin_dir: &Path, name: &str, target: &Path) -> Result<(), String> {
    let path = bin_dir.join(name);
    let _ = fs::remove_file(&path);
    std::os::unix::fs::symlink(target, &path)
        .map_err(|e| format!("cannot link {}: {e}", path.display()))
}

#[cfg(windows)]
fn link(bin_dir: &Path, name: &str, target: &Path) -> Result<(), String> {
    // Windows has no usable symlinks without elevation, so npm's trick: a
    // .cmd shim that forwards every argument to the real script.
    let path = bin_dir.join(format!("{name}.cmd"));
    let target = target.to_string_lossy().replace('/', "\\");
    let body = format!("@\"%~dp0{target}\" %*\r\n");
    fs::write(&path, body).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Run the lifecycle scripts of every installed package, dependencies first.
pub fn run_lifecycle(project: &Path, installed: &[Installed]) {
    let mut ran = 0usize;
    let mut failed: Vec<String> = Vec::new();

    for pkg in installed {
        let manifest = match pkg.manifest() {
            Some(m) => m,
            None => continue,
        };
        let scripts = match manifest.get("scripts").and_then(|s| s.as_obj()) {
            Some(s) => s,
            None => continue,
        };
        for event in LIFECYCLE {
            let body = scripts
                .iter()
                .find(|(key, _)| key == event)
                .and_then(|(_, value)| value.as_str());
            let body = match body {
                Some(b) if !b.trim().is_empty() => b,
                _ => continue,
            };
            ran += 1;
            match run_one(project, pkg, event, body) {
                Ok(()) => ui::status(ui::CHECK, &format!("{} {}", pkg.name, event), "done"),
                Err(e) => {
                    ui::miss(&format!("{} {} failed ({e})", pkg.name, event));
                    failed.push(pkg.name.clone());
                }
            }
        }
    }

    if ran > 0 && failed.is_empty() {
        ui::info(&format!("{ran} lifecycle script(s) ran"));
    } else if !failed.is_empty() {
        ui::warn(&format!(
            "{} package(s) failed a lifecycle script: {}",
            failed.len(),
            failed.join(", ")
        ));
        ui::info("the packages are installed but may need `nth i --ignore-scripts` or a manual build");
    }
}

fn run_one(project: &Path, pkg: &Installed, event: &str, body: &str) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        // npm runs scripts as `cmd.exe /d /s /c "<script>"`; the flags
        // matter, /S keeps cmd from stripping the quotes node needs.
        let mut c = Command::new("cmd");
        c.args(["/D", "/S", "/C"]).arg(body);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(body);
        c
    };

    let path = env_path(&[
        &pkg.node_modules.join(".bin"),
        &project.join("node_modules").join(".bin"),
    ]);
    let path_key = if cfg!(windows) { "Path" } else { "PATH" };
    command
        .current_dir(&pkg.dir)
        .env(path_key, path)
        .env("npm_lifecycle_event", event)
        .env("npm_package_name", &pkg.name)
        .env("INIT_CWD", project);

    let output = command
        .output()
        .map_err(|e| format!("cannot start script: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    Err(match output.status.code() {
        Some(code) if last.is_empty() => format!("exit {code}"),
        Some(code) => format!("exit {code}: {last}"),
        None => format!("killed: {last}"),
    })
}

/// The current PATH with the package bin folders in front.
fn env_path(dirs: &[&PathBuf]) -> std::ffi::OsString {
    let mut parts: Vec<PathBuf> = dirs.iter().map(|d| d.to_path_buf()).collect();
    if let Some(existing) = std::env::var_os("PATH") {
        parts.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(parts).unwrap_or_else(|_| std::ffi::OsString::from(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nth-scripts-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_every_bin_shape() {
        let manifest = Json::parse(r#"{"bin":{"next":"dist/bin/next","next-dev":"dev.js"}}"#).unwrap();
        let found = bins(&manifest, "next");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], ("next".to_string(), "dist/bin/next".to_string()));

        let single = Json::parse(r#"{"bin":"cli.js"}"#).unwrap();
        assert_eq!(bins(&single, "@scope/tool"), vec![("tool".to_string(), "cli.js".to_string())]);

        let none = Json::parse(r#"{"name":"x"}"#).unwrap();
        assert!(bins(&none, "x").is_empty());
    }

    #[test]
    fn links_bins_and_runs_scripts() {
        let root = temp_dir("bins");
        let nm = root.join("node_modules");

        // package with a bin and a postinstall that uses it. the script
        // lives in a file so the test does not depend on shell quoting.
        let pkg_dir = nm.join("widget");
        fs::create_dir_all(pkg_dir.join("bin")).unwrap();
        fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"widget","version":"1.0.0","bin":{"widget":"bin/cli.js"},
                "scripts":{"postinstall":"node build.js"}}"#,
        )
        .unwrap();
        fs::write(pkg_dir.join("bin/cli.js"), "#!/usr/bin/env node\n").unwrap();
        fs::write(
            pkg_dir.join("build.js"),
            "require('fs').writeFileSync('ran.txt', 'built');\n",
        )
        .unwrap();

        let installed = vec![Installed {
            name: "widget".into(),
            dir: pkg_dir.clone(),
            node_modules: nm.clone(),
        }];

        assert_eq!(link_bins(&installed).unwrap(), 1);
        let link = nm.join(".bin/widget");
        assert!(link.exists() || nm.join(".bin/widget.cmd").exists());

        run_lifecycle(&root, &installed);
        let marker = pkg_dir.join("ran.txt");
        assert!(marker.is_file(), "postinstall did not run in the package dir");
        assert_eq!(fs::read_to_string(&marker).unwrap(), "built");
        let _ = fs::remove_dir_all(root);
    }
}
