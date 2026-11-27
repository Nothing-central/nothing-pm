//! CLI argument parsing. Hand written — the command surface is small enough
//! that a derive-based parser would be more dependency than value.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Npm,
    Jsr,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Npm => "npm",
            Source::Jsr => "jsr",
        }
    }

    pub fn parse(s: &str) -> Option<Source> {
        match s {
            "npm" => Some(Source::Npm),
            "jsr" => Some(Source::Jsr),
            _ => None,
        }
    }
}

/// A single requested package, e.g. `axios`, `axios@2.2.4` or `jsr @hono/hono`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageSpec {
    pub name: String,
    /// `None` means "whatever the registry considers latest".
    pub range: Option<String>,
    pub source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearPart {
    /// `nth clear <pkg>` — remove everything.
    All,
    /// `nth clear <pkg> prev` — drop the saved previous version.
    Prev,
    /// `nth clear <pkg> new` — drop the current version, promoting prev.
    New,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Install {
        specs: Vec<PackageSpec>,
        forceall: bool,
        ignore_scripts: bool,
    },
    InstallAll {
        forceall: bool,
        ignore_scripts: bool,
    },
    Uninstall { names: Vec<String> },
    Add { specs: Vec<PackageSpec> },
    UpdateDatabase,
    CheckUpdates,
    ViewDb,
    Delete { name: String },
    Clear { name: String, part: ClearPart },
    Info,
    Lock { json: bool },
    Init,
    /// `nth save` — write this project's install recipe to a script.
    Save { file: Option<String> },
    /// `nth run <script>` / `nth do <script>` — replay a saved script.
    RunScript { file: String },
    Help,
    Version,
}

/// True when the user asked nth to answer every prompt for itself.
fn yes_requested(args: &[String]) -> bool {
    take_flag(args, &["--yes", "-y"]) || yes_from_env()
}

fn yes_from_env() -> bool {
    match std::env::var("NTH_YES") {
        Ok(v) => !matches!(v.trim().to_lowercase().as_str(), "" | "0" | "false" | "no"),
        Err(_) => false,
    }
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut iter = args.iter();
    let sub = match iter.next() {
        Some(s) => s.as_str(),
        None => return Ok(Command::Help),
    };
    let rest: Vec<String> = iter.cloned().collect();
    // `-y` / `--yes` / NTH_YES switch every later prompt to auto-answer.
    crate::ui::set_yes(yes_requested(&rest));

    match sub {
        "install" | "i" | "add-install" => {
            let forceall = take_flag(&rest, &["--forceall", "--force-all", "-f"]);
            let ignore_scripts = take_flag(
                &rest,
                &["--ignore-scripts", "--no-scripts", "--noscripts"],
            );
            let positional: Vec<String> = rest
                .iter()
                .filter(|a| !is_flag(a))
                .cloned()
                .collect();
            if positional.is_empty() {
                Ok(Command::InstallAll {
                    forceall,
                    ignore_scripts,
                })
            } else {
                if forceall {
                    return Err("--forceall only applies when installing from package.json".into());
                }
                Ok(Command::Install {
                    specs: parse_specs(&positional)?,
                    forceall: false,
                    ignore_scripts,
                })
            }
        }
        "add" | "fetch" => {
            let positional: Vec<String> =
                rest.iter().filter(|a| !is_flag(a)).cloned().collect();
            if positional.is_empty() {
                return Err("nth add needs at least one package, e.g. `nth add axios, express`"
                    .into());
            }
            Ok(Command::Add {
                specs: parse_specs(&positional)?,
            })
        }
        "update" => match rest.first().map(|s| s.as_str()) {
            Some("database") | Some("db") | None => Ok(Command::UpdateDatabase),
            Some(other) => Err(format!("unknown update target '{other}' — try `nth update database`")),
        },
        "check" => match rest.first().map(|s| s.as_str()) {
            Some("updates") | Some("update") | None => Ok(Command::CheckUpdates),
            Some(other) => Err(format!("unknown check target '{other}' — try `nth check updates`")),
        },
        "view" => match rest.first().map(|s| s.as_str()) {
            Some("db") | Some("database") | None => Ok(Command::ViewDb),
            Some(other) => Err(format!("unknown view target '{other}' — try `nth view db`")),
        },
        // npm-style removal from the project; the store copy is kept.
        "uninstall" | "uni" | "remove" | "rm" => Ok(Command::Uninstall {
            names: parse_names(&rest)?,
        }),
        "delete" => {
            let name = rest.first().ok_or("nth delete needs a package name")?;
            Ok(Command::Delete {
                name: name.clone(),
            })
        }
        "clear" => {
            let name = rest.first().ok_or("nth clear needs a package name")?;
            let part = match rest.get(1).map(|s| s.as_str()) {
                None => ClearPart::All,
                Some("prev") | Some("previous") => ClearPart::Prev,
                Some("new") | Some("current") => ClearPart::New,
                Some(other) => {
                    return Err(format!(
                        "unknown clear mode '{other}' — expected `prev` or `new`"
                    ))
                }
            };
            Ok(Command::Clear {
                name: name.clone(),
                part,
            })
        }
        "info" => Ok(Command::Info),
        "lock" | "lockfile" => match rest.first().map(|s| s.as_str()) {
            None => Ok(Command::Lock { json: false }),
            Some("json") => Ok(Command::Lock { json: true }),
            Some(other) => Err(format!(
                "unknown lock target '{other}' — try `nth lock` or `nth lock json`"
            )),
        },
        "init" => Ok(Command::Init),
        "save" => {
            let file = rest
                .iter()
                .find(|a| !is_flag(a))
                .cloned();
            if let Some(unknown) = rest.iter().find(|a| is_flag(a)) {
                return Err(format!(
                    "unknown save option '{unknown}' — try `nth save [file]`"
                ));
            }
            Ok(Command::Save { file })
        }
        "run" | "do" => {
            let positional: Vec<String> =
                rest.iter().filter(|a| !is_flag(a)).cloned().collect();
            match positional.len() {
                0 => Err("nth run needs a script, e.g. `nth run nth-setup.txt`".into()),
                1 => Ok(Command::RunScript {
                    file: positional[0].clone(),
                }),
                n => Err(format!(
                    "nth run takes one script file, got {n} — try `nth run <file>`"
                )),
            }
        }
        "help" | "--help" | "-h" => Ok(Command::Help),
        "version" | "--version" | "-V" => Ok(Command::Version),
        other => Err(format!("unknown command '{other}' — run `nth help`")),
    }
}

fn is_flag(arg: &str) -> bool {
    arg.starts_with('-')
}

fn take_flag(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|a| names.contains(&a.as_str()))
}

/// Flattens `axios, jsr @hono/hono, express` into three specs.
///
/// The `jsr` keyword is an inline prefix for the package immediately after
/// it; everything else defaults to npm. Both argv shapes work: the caller's
/// shell may hand us `["jsr", "@hono/hono"]` or `["jsr @hono/hono"]`.
pub fn parse_specs(tokens: &[String]) -> Result<Vec<PackageSpec>, String> {
    let mut flat: Vec<String> = Vec::new();
    for token in tokens {
        for part in token.split(',') {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                flat.push(trimmed.to_string());
            }
        }
    }

    let mut specs = Vec::new();
    let mut pending_jsr = false;
    for token in flat {
        if token == "jsr" {
            pending_jsr = true;
            continue;
        }
        // "jsr @hono/hono" arriving as a single argv entry
        let (token, jsr_here) = match token.strip_prefix("jsr ") {
            Some(rest) => (rest.trim().to_string(), true),
            None => (token, false),
        };
        if token.is_empty() {
            continue;
        }
        let mut spec = parse_spec(&token)?;
        if pending_jsr || jsr_here {
            spec.source = Source::Jsr;
            pending_jsr = false;
        }
        specs.push(spec);
    }
    if pending_jsr {
        return Err("`jsr` must be followed by a package name".into());
    }
    if specs.is_empty() {
        return Err("no packages given".into());
    }
    Ok(specs)
}

/// Flattens `axios, @scope/pkg@1.2.3` into bare package names for uninstall.
pub fn parse_names(tokens: &[String]) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::new();
    for token in tokens {
        for part in token.split(',') {
            let raw = part.trim();
            if raw.is_empty() {
                continue;
            }
            let name = strip_version(raw);
            validate_name(&name)?;
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if names.is_empty() {
        return Err("nth uninstall needs at least one package name".into());
    }
    Ok(names)
}

fn strip_version(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix('@') {
        return match rest.find('@') {
            Some(i) => format!("@{}", &rest[..i]),
            None => raw.to_string(),
        };
    }
    match raw.find('@') {
        Some(i) => raw[..i].to_string(),
        None => raw.to_string(),
    }
}

fn parse_spec(raw: &str) -> Result<PackageSpec, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty package name".into());
    }
    // Scoped names keep their leading '@', so only look for a second '@'.
    let (name, range) = if let Some(rest) = raw.strip_prefix('@') {
        match rest.find('@') {
            Some(i) => (format!("@{}", &rest[..i]), Some(rest[i + 1..].to_string())),
            None => (raw.to_string(), None),
        }
    } else {
        match raw.find('@') {
            Some(i) => (raw[..i].to_string(), Some(raw[i + 1..].to_string())),
            None => (raw.to_string(), None),
        }
    };

    validate_name(&name)?;
    let range = match range {
        Some(r) if r.trim().is_empty() => None,
        Some(r) => Some(r.trim().to_string()),
        None => None,
    };

    Ok(PackageSpec {
        name,
        range,
        source: Source::Npm,
    })
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty package name".into());
    }
    if name.len() > 214 {
        return Err(format!("package name '{name}' is too long"));
    }
    if name.starts_with('@') {
        let rest = &name[1..];
        let (scope, pkg) = rest
            .split_once('/')
            .ok_or_else(|| format!("scoped package '{name}' must look like @scope/name"))?;
        if scope.is_empty() || pkg.is_empty() {
            return Err(format!("invalid scoped package name '{name}'"));
        }
        check_name_chars(scope, name)?;
        check_name_chars(pkg, name)?;
    } else {
        check_name_chars(name, name)?;
    }
    Ok(())
}

fn check_name_chars(part: &str, full: &str) -> Result<(), String> {
    for ch in part.chars() {
        let ok = ch.is_ascii_lowercase()
            || ch.is_ascii_digit()
            || ch == '-'
            || ch == '_'
            || ch == '.'
            || ch == '~';
        if !ok {
            return Err(format!(
                "invalid package name '{full}': unexpected character '{ch}'"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_install_with_mixed_registries() {
        let cmd = parse(&args("install axios, express, jsr @hono/hono")).unwrap();
        match cmd {
            Command::Install { specs, .. } => {
                assert_eq!(specs.len(), 3);
                assert_eq!(specs[0].source, Source::Npm);
                assert_eq!(specs[1].name, "express");
                assert_eq!(specs[2].name, "@hono/hono");
                assert_eq!(specs[2].source, Source::Jsr);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn jsr_prefix_as_separate_argv() {
        let cmd = parse(&args("add baileys, jsr @hono/hono, elysia")).unwrap();
        match cmd {
            Command::Add { specs } => {
                assert_eq!(specs[1].source, Source::Jsr);
                assert_eq!(specs[2].source, Source::Npm);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_pinned_versions() {
        let cmd = parse(&args("install axios@2.2.4 jsr @std/path@1.0.0")).unwrap();
        match cmd {
            Command::Install { specs, .. } => {
                assert_eq!(specs[0].range.as_deref(), Some("2.2.4"));
                assert_eq!(specs[1].name, "@std/path");
                assert_eq!(specs[1].range.as_deref(), Some("1.0.0"));
                assert_eq!(specs[1].source, Source::Jsr);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn bare_install_means_everything() {
        assert_eq!(
            parse(&args("i")).unwrap(),
            Command::InstallAll {
                forceall: false,
                ignore_scripts: false
            }
        );
        assert_eq!(
            parse(&args("i --forceall")).unwrap(),
            Command::InstallAll {
                forceall: true,
                ignore_scripts: false
            }
        );
        assert_eq!(
            parse(&args("i --ignore-scripts")).unwrap(),
            Command::InstallAll {
                forceall: false,
                ignore_scripts: true
            }
        );
    }

    #[test]
    fn parses_uninstall() {
        let cmd = parse(&args("uninstall axios, @scope/pkg@1.2.3 express")).unwrap();
        match cmd {
            Command::Uninstall { names } => {
                assert_eq!(names, vec!["axios", "@scope/pkg", "express"]);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            parse(&args("remove axios")).unwrap(),
            Command::Uninstall {
                names: vec!["axios".into()]
            }
        );
        assert!(parse(&args("uninstall")).is_err());
    }

    #[test]
    fn parses_clear_modes() {
        assert_eq!(
            parse(&args("clear axios prev")).unwrap(),
            Command::Clear {
                name: "axios".into(),
                part: ClearPart::Prev
            }
        );
        assert_eq!(
            parse(&args("clear axios")).unwrap(),
            Command::Clear {
                name: "axios".into(),
                part: ClearPart::All
            }
        );
    }

    #[test]
    fn parses_lock_modes() {
        assert_eq!(parse(&args("lock")).unwrap(), Command::Lock { json: false });
        assert_eq!(
            parse(&args("lock json")).unwrap(),
            Command::Lock { json: true }
        );
        assert!(parse(&args("lock yaml")).is_err());
    }

    #[test]
    fn rejects_bad_names() {
        assert!(parse(&args("install UPPER")).is_err());
        assert!(parse(&args("install @scope")).is_err());
    }

    #[test]
    fn yes_flag_is_not_a_package() {
        let cmd = parse(&args("install axios -y")).unwrap();
        match cmd {
            Command::Install { specs, .. } => {
                assert_eq!(specs.len(), 1);
                assert_eq!(specs[0].name, "axios");
            }
            other => panic!("unexpected {other:?}"),
        }
        crate::ui::set_yes(false);
    }

    #[test]
    fn parses_save_and_run() {
        assert_eq!(
            parse(&args("save")).unwrap(),
            Command::Save { file: None }
        );
        assert_eq!(
            parse(&args("save my-setup.txt")).unwrap(),
            Command::Save {
                file: Some("my-setup.txt".into())
            }
        );
        assert_eq!(
            parse(&args("run my-setup.txt")).unwrap(),
            Command::RunScript {
                file: "my-setup.txt".into()
            }
        );
        assert_eq!(
            parse(&args("do my-setup.txt")).unwrap(),
            Command::RunScript {
                file: "my-setup.txt".into()
            }
        );
        assert!(parse(&args("run")).is_err());
    }
}
