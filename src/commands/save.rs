//! `nth save` and `nth run <script>`.
//!
//! A script is a plain-text recipe: one `name@range source` per line, with
//! `#` comments. `nth save` writes the current project's roots into it; when
//! the project has an agree.db whose roots match package.json, every locked
//! version is pinned exactly (`name@1.2.3`) so the replay lands on the same
//! tree without asking the registry to re-resolve anything.
//!
//! `nth run <script>` replays a recipe in the current directory: it creates a
//! package.json if needed, adds the listed dependencies, installs them and —
//! because running a saved script *is* the confirmation — answers every
//! prompt automatically (yes mode). One command rebuilds a whole app.

use super::{context, ensure_packages, install_plan};
use crate::cli::Source;
use crate::install as installer;
use crate::lockfile::Lockfile;
use crate::project::{self, Manifest};
use crate::resolver::Root;
use crate::ui;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_SCRIPT: &str = "nth-setup.txt";

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptLine {
    pub name: String,
    pub range: Option<String>,
    pub source: Source,
}

impl ScriptLine {
    pub fn spec(&self) -> String {
        match &self.range {
            Some(r) => format!("{}@{} {}", self.name, r, self.source.as_str()),
            None => format!("{} {}", self.name, self.source.as_str()),
        }
    }
}

/// Parse a recipe. Blank lines and `#` comments are ignored. Accepted forms:
///   react@19.0.0                 (npm, exact)
///   next@^15                     (npm, range)
///   react@latest npm             (explicit source)
///   @scope/pkg@1.0.0 jsr         (scoped names keep their leading @)
///   jsr:std/path@1.0.0           (prefix form)
pub fn parse_script(text: &str) -> Result<Vec<ScriptLine>, String> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (spec, source_arg) = match line.split_once(char::is_whitespace) {
            Some((a, b)) => (a, Some(b.trim())),
            None => (line, None),
        };
        let mut source = match source_arg {
            Some(s) => {
                if s.contains(char::is_whitespace) {
                    return Err(format!(
                        "line {}: unexpected extra text '{s}'",
                        i + 1
                    ));
                }
                Source::parse(s).ok_or_else(|| {
                    format!("line {}: unknown source '{s}' — expected npm or jsr", i + 1)
                })?
            }
            None => Source::Npm,
        };
        let mut spec = spec.trim_start_matches("npm:");
        if let Some(rest) = spec.strip_prefix("jsr:") {
            source = Source::Jsr;
            spec = rest;
        }
        // Split the last '@' that is not the start of a scoped name.
        let at = spec.rfind('@').filter(|p| *p > 0);
        let (name, range) = match at {
            Some(p) => (&spec[..p], Some(spec[p + 1..].to_string())),
            None => (spec, None),
        };
        let first_ok = name
            .chars()
            .next()
            .map(|c| c.is_alphanumeric() || c == '@')
            .unwrap_or(false);
        let scoped_ok = !name.starts_with('@') || name.contains('/');
        if !first_ok || !scoped_ok {
            return Err(format!("line {}: missing package name in '{line}'", i + 1));
        }
        if let Some(r) = &range {
            if r.is_empty() {
                return Err(format!("line {}: empty version after '@'", i + 1));
            }
        }
        out.push(ScriptLine {
            name: name.to_string(),
            range,
            source,
        });
    }
    Ok(out)
}

fn script_path(file: Option<&str>) -> PathBuf {
    match file {
        Some(f) => PathBuf::from(f),
        None => PathBuf::from(DEFAULT_SCRIPT),
    }
}

/// `nth save [file]`: write this project's install recipe.
pub fn run_save(file: Option<String>) -> Result<(), String> {
    let ctx = context()?;
    let project = project::project_root()?;
    let manifest_path = project.join("package.json");
    if !manifest_path.is_file() {
        return Err(
            "no package.json here — run nth save inside a project (or `nth init` first)".into(),
        );
    }
    let manifest = Manifest::read(&manifest_path)?;
    if manifest.dependencies.is_empty() {
        ui::warn("package.json lists no dependencies — nothing worth saving yet");
    }

    // Exact pins come from agree.db, but only when it still matches
    // package.json — a stale lock would freeze the wrong tree.
    let pins: Vec<(String, String, Source)> = match Lockfile::open(&project) {
        Ok(lock) => {
            let roots: Vec<Root> = manifest
                .dependencies
                .iter()
                .map(|(name, range)| Root {
                    name: name.clone(),
                    range: Some(range.clone()),
                    source: super::stored_source(&ctx, name),
                })
                .collect();
            match lock.matches(&roots) {
                Ok(true) => lock.roots()?,
                _ => Vec::new(),
            }
        }
        Err(_) => Vec::new(),
    };

    let path = script_path(file.as_deref());
    let mut text = String::new();
    text.push_str("# nth setup script — recreate this project with\n");
    text.push_str("#   nth run ");
    text.push_str(&path.display().to_string());
    text.push('\n');
    if let Some(name) = &manifest.name {
        text.push_str(&format!("# project: {name}\n"));
    }
    text.push_str(&format!("# saved by nth {}\n\n", env!("CARGO_PKG_VERSION")));

    let mut written = 0usize;
    for (name, range) in &manifest.dependencies {
        let pin = pins.iter().find(|(pn, _, _)| pn == name);
        let (spec_range, source) = match pin {
            Some((_, v, s)) => (Some(v.clone()), *s),
            None => (Some(range.clone()), super::stored_source(&ctx, name)),
        };
        text.push_str(&ScriptLine {
            name: name.clone(),
            range: spec_range,
            source,
        }.spec());
        text.push('\n');
        written += 1;
    }

    fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    ui::blank();
    ui::ok(&format!("saved {written} package(s) to {}", path.display()));
    ui::info(&format!(
        "recreate anywhere with: nth run {}",
        path.display()
    ));
    Ok(())
}

/// `nth run <script>`: replay a recipe here, answering every prompt itself.
pub fn run_replay(file: String) -> Result<(), String> {
    let path = PathBuf::from(&file);
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let lines = parse_script(&text)?;
    if lines.is_empty() {
        return Err(format!("{} has no packages in it", path.display()));
    }

    // Running a saved script is the confirmation; never block on prompts.
    ui::set_yes(true);

    let ctx = context()?;
    let project = project::project_root()?;
    let manifest_path = project.join("package.json");
    if !manifest_path.is_file() {
        bootstrap_manifest(&manifest_path, &project)?;
    }

    ui::info(&format!(
        "replaying {} — {} package(s)",
        path.display(),
        lines.len()
    ));
    ui::blank();

    let roots: Vec<Root> = lines
        .iter()
        .map(|l| Root {
            name: l.name.clone(),
            range: l.range.clone(),
            source: l.source,
        })
        .collect();

    let (plan, downloaded) = ensure_packages(&ctx, &roots, false)?;
    let node_modules = installer::node_modules(&project);

    ui::blank();
    ui::info("arranging folders...");
    let count = install_plan(&ctx, &plan, &node_modules, &downloaded, false)?;

    // Write the deps into package.json so the project is a normal one from
    // here on — `nth i` works without the script.
    let mut manifest = Manifest::read(&manifest_path)?;
    for l in &lines {
        let range = match &l.range {
            Some(r) => r.clone(),
            None => "*".to_string(),
        };
        manifest.add_dependency(&l.name, &range)?;
    }

    super::install::finish_install(&project, &node_modules, &plan, false)?;
    super::lock::record(&project, &roots, &plan)?;

    ui::blank();
    ui::ok(&format!("{count} packages installed from {}", path.display()));
    ui::info("the project is ready — `nth i` repeats this without the script");
    Ok(())
}

fn bootstrap_manifest(path: &Path, project: &Path) -> Result<(), String> {
    let name = project
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "app".to_string());
    let json = format!(
        "{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\",\n  \"dependencies\": {{}}\n}}\n"
    );
    fs::write(path, json).map_err(|e| format!("cannot create package.json: {e}"))?;
    ui::info("created package.json for this project");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_pinned_lines() {
        let text = "# comment\n\nreact@19.0.0\nnext@^15 npm\n";
        let lines = parse_script(text).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].name, "react");
        assert_eq!(lines[0].range.as_deref(), Some("19.0.0"));
        assert_eq!(lines[0].source, Source::Npm);
        assert_eq!(lines[1].name, "next");
        assert_eq!(lines[1].source, Source::Npm);
    }

    #[test]
    fn parses_scoped_and_jsr_names() {
        let lines = parse_script("@babel/core@7.26.0\njsr:std/path@1.0.0 jsr\n").unwrap();
        assert_eq!(lines[0].name, "@babel/core");
        assert_eq!(lines[0].range.as_deref(), Some("7.26.0"));
        assert_eq!(lines[1].name, "std/path");
        assert_eq!(lines[1].source, Source::Jsr);
    }

    #[test]
    fn rejects_unknown_source() {
        assert!(parse_script("react@1.0.0 pnpm").is_err());
        assert!(parse_script("@1.0.0").is_err());
    }

    #[test]
    fn round_trips_specs() {
        let lines = parse_script("react@19.0.0\nfoo jsr\n").unwrap();
        assert_eq!(lines[0].spec(), "react@19.0.0 npm");
        assert_eq!(lines[1].spec(), "foo jsr");
        let again = parse_script(&format!("{}\n{}\n", lines[0].spec(), lines[1].spec())).unwrap();
        assert_eq!(again, lines);
    }
}
