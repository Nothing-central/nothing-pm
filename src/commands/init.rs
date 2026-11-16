//! `nth init` — create a starter package.json.
//!
//! Mirrors what `npm init -y` writes so an empty directory becomes a usable
//! project in one command. If a package.json already exists, init is a no-op:
//! package.json belongs to the user and nothing rewrites it uninvited.

use crate::json::Json;
use crate::ui;
use std::path::Path;

pub fn run() -> Result<(), String> {
    let cwd = std::env::current_dir()
        .map_err(|e| format!("cannot read current directory: {e}"))?;
    let target = cwd.join("package.json");

    if target.exists() {
        ui::warn(&format!(
            "{} already exists — leaving it untouched",
            target.display()
        ));
        return Ok(());
    }

    let doc = Json::obj(vec![
        ("name", Json::s(&package_name(&cwd))),
        ("version", Json::s("1.0.0")),
        ("description", Json::s("")),
        ("main", Json::s("index.js")),
        (
            "scripts",
            Json::obj(vec![(
                "test",
                Json::s("echo \"Error: no test specified\" && exit 1"),
            )]),
        ),
        ("keywords", Json::Arr(Vec::new())),
        ("author", Json::s("")),
        ("license", Json::s("ISC")),
    ]);

    std::fs::write(&target, doc.to_pretty())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    ui::ok(&format!("created {}", target.display()));
    Ok(())
}

/// Directory name → valid npm package name: lowercase, url-safe characters
/// only, no leading or trailing punctuation.
fn package_name(dir: &Path) -> String {
    let raw = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in raw.chars().flat_map(|c| c.to_lowercase()) {
        let valid = ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '.';
        if valid {
            out.push(ch);
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }

    let trimmed = out
        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_string();

    if trimmed.is_empty() {
        "nothing-project".to_string()
    } else if trimmed.len() > 214 {
        trimmed[..214].trim_end_matches('-').to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn sanitises_directory_names() {
        let cases = [
            ("/home/x/My Cool App!", "my-cool-app"),
            ("/tmp/axios", "axios"),
            ("/tmp/42-thing", "42-thing"),
            ("/tmp/.hidden", "hidden"),
            ("/tmp/__weird__", "weird"),
        ];
        for (dir, expected) in cases {
            assert_eq!(package_name(&PathBuf::from(dir)), expected, "for {dir}");
        }
        assert_eq!(package_name(Path::new("/")), "nothing-project");
    }

    #[test]
    fn generated_manifest_is_valid_json() {
        let doc = Json::obj(vec![
            ("name", Json::s("demo")),
            ("version", Json::s("1.0.0")),
            (
                "scripts",
                Json::obj(vec![(
                    "test",
                    Json::s("echo \"Error: no test specified\" && exit 1"),
                )]),
            ),
        ]);
        let parsed = Json::parse(&doc.to_pretty()).unwrap();
        assert_eq!(parsed.str_at("name"), Some("demo"));
        assert_eq!(
            parsed.str_path(&["scripts", "test"]),
            Some("echo \"Error: no test specified\" && exit 1")
        );
    }
}
