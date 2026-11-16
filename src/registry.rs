//! Registry clients for npm and jsr.
//!
//! jsr is consumed through its npm-compatibility endpoint (`npm.jsr.io`),
//! which serves a normal packument and a normal tarball. That keeps one
//! fetch path for both registries: `@hono/hono` becomes `@jsr/hono__hono`
//! on the wire and comes back as `@hono/hono` in the store.

use crate::cli::Source;
use crate::http::{Client, HttpError};
use crate::semver::{self, Range, Version};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepRef {
    pub name: String,
    pub range: String,
    pub source: Source,
}

#[derive(Clone, Debug)]
pub struct VersionMeta {
    pub version: Version,
    pub tarball: String,
    pub integrity: Option<String>,
    pub deps: Vec<DepRef>,
    /// Install if you can, keep going if you cannot (npm semantics).
    pub optional: Vec<DepRef>,
    /// npm 7+ installs peers alongside the package that asks for them.
    pub peer: Vec<DepRef>,
    /// Platform guards: `os`, `cpu` and `libc` from the version's metadata.
    pub os: Vec<String>,
    pub cpu: Vec<String>,
    pub libc: Vec<String>,
}

/// The platform this binary is running on, in npm's vocabulary
/// (`linux`/`darwin`/`win32`, `x64`/`arm64`, `glibc`/`musl`).
#[derive(Clone, Debug)]
pub struct Platform {
    pub os: String,
    pub cpu: String,
    pub libc: Option<String>,
}

impl Platform {
    pub fn current() -> Platform {
        let os = match std::env::consts::OS {
            "macos" => "darwin".to_string(),
            other => other.to_string(),
        };
        let cpu = match std::env::consts::ARCH {
            "x86" => "ia32".to_string(),
            "x86_64" => "x64".to_string(),
            "aarch64" => "arm64".to_string(),
            "powerpc64" => "ppc64".to_string(),
            other => other.to_string(),
        };
        let libc = if os == "linux" {
            detect_linux_libc().map(|s| s.to_string())
        } else {
            None
        };
        Platform { os, cpu, libc }
    }

    pub fn label(&self) -> String {
        match &self.libc {
            Some(libc) => format!("{}/{} ({})", self.os, self.cpu, libc),
            None => format!("{}/{}", self.os, self.cpu),
        }
    }
}

/// A package binary built against musl will not load on glibc, so this one
/// has to be right. Every mainstream Linux image has one of these loaders.
fn detect_linux_libc() -> Option<&'static str> {
    for dir in ["/lib", "/usr/lib", "/lib64"] {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("ld-musl-") {
                return Some("musl");
            }
        }
    }
    Some("glibc")
}

/// True when this version can run here. An empty guard means "no opinion";
/// `!value` entries exclude exactly that value.
pub fn supports(meta: &VersionMeta, platform: &Platform) -> bool {
    field_allows(&meta.os, &platform.os)
        && field_allows(&meta.cpu, &platform.cpu)
        && match &platform.libc {
            Some(libc) => field_allows(&meta.libc, libc),
            None => true,
        }
}

fn field_allows(values: &[String], current: &str) -> bool {
    if values.is_empty() {
        return true;
    }
    let mut has_allow_entry = false;
    let mut allowed = false;
    for value in values {
        if let Some(negated) = value.strip_prefix('!') {
            if negated == current {
                return false;
            }
        } else {
            has_allow_entry = true;
            if value == current {
                allowed = true;
            }
        }
    }
    // A list of only exclusions (["!win32"]) allows everything else.
    if has_allow_entry {
        allowed
    } else {
        true
    }
}

fn string_list(value: Option<&crate::json::Json>) -> Vec<String> {
    let items = match value.and_then(|v| v.as_arr()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    items
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect()
}

/// One `{name: range}` map from a version's metadata.
fn dep_refs(meta: &crate::json::Json, key: &str) -> Vec<DepRef> {
    let entries = match meta.get(key).and_then(|d| d.as_obj()) {
        Some(o) => o,
        None => return Vec::new(),
    };
    entries
        .iter()
        .map(|(dep_name, dep_range)| {
            let dep_range = dep_range.as_str().unwrap_or("*");
            let (name, source) = match dep_name.strip_prefix("@jsr/") {
                Some(_) => (jsr_real_name(dep_name), Source::Jsr),
                None => (dep_name.clone(), Source::Npm),
            };
            DepRef {
                name,
                range: dep_range.to_string(),
                source,
            }
        })
        .collect()
}

/// `peerDependencies` minus everything marked optional in
/// `peerDependenciesMeta` — those are the peer shell a package can live
/// without.
fn peer_refs(meta: &crate::json::Json) -> Vec<DepRef> {
    let optional_meta = meta.get("peerDependenciesMeta").and_then(|m| m.as_obj());
    dep_refs(meta, "peerDependencies")
        .into_iter()
        .filter(|dep| {
            let marked_optional = optional_meta
                .map(|entries| {
                    entries.iter().any(|(name, value)| {
                        name == &dep.name
                            && matches!(value.get("optional"), Some(crate::json::Json::Bool(true)))
                    })
                })
                .unwrap_or(false);
            !marked_optional
        })
        .collect()
}

impl VersionMeta {
    pub fn is_prerelease(&self) -> bool {
        self.version.is_prerelease()
    }
}

#[derive(Clone, Debug)]
pub struct Packument {
    pub dist_tags: Vec<(String, String)>,
    pub versions: Vec<VersionMeta>,
}

impl Packument {
    pub fn version(&self, v: &Version) -> Option<&VersionMeta> {
        self.versions.iter().find(|m| &m.version == v)
    }

    pub fn tag(&self, tag: &str) -> Option<&VersionMeta> {
        let text = self
            .dist_tags
            .iter()
            .find(|(k, _)| k == tag)
            .map(|(_, v)| v)?;
        let parsed = Version::parse(text).ok()?;
        self.version(&parsed)
    }

    pub fn latest(&self) -> Option<&VersionMeta> {
        if let Some(m) = self.tag("latest") {
            return Some(m);
        }
        // Fall back to the highest stable version, then the highest overall.
        let stable = self.versions.iter().filter(|m| !m.is_prerelease()).max_by(|a, b| a.version.cmp(&b.version));
        stable.or_else(|| self.versions.iter().max_by(|a, b| a.version.cmp(&b.version)))
    }

    pub fn max_satisfying(&self, range: &Range) -> Option<&VersionMeta> {
        semver::max_satisfying(self.versions.iter().map(|m| &m.version), range)
            .and_then(|v| self.version(&v))
    }
}

#[derive(Debug)]
pub enum RegistryError {
    Network(HttpError),
    NotFound(String),
    BadResponse(String),
}

impl RegistryError {
    pub fn message(&self) -> String {
        match self {
            RegistryError::Network(e) => e.message(),
            RegistryError::NotFound(n) => format!("package '{n}' does not exist on this registry"),
            RegistryError::BadResponse(m) => format!("unexpected registry response: {m}"),
        }
    }

}

#[derive(Debug)]
pub enum ResolveError {
    /// The requested version is not published (spec §9).
    VersionMissing { requested: String, latest: Option<String> },
    /// No published version satisfies the range.
    NoMatch { range: String, latest: Option<String> },
}

impl ResolveError {
    pub fn message(&self) -> String {
        match self {
            ResolveError::VersionMissing { requested, latest } => match latest {
                Some(l) => format!("version {requested} does not exist yet (latest is {l})"),
                None => format!("version {requested} does not exist yet"),
            },
            ResolveError::NoMatch { range, latest } => match latest {
                Some(l) => format!("no published version matches '{range}' (latest is {l})"),
                None => format!("no published version matches '{range}'"),
            },
        }
    }

    pub fn latest(&self) -> Option<&str> {
        match self {
            ResolveError::VersionMissing { latest, .. } | ResolveError::NoMatch { latest, .. } => {
                latest.as_deref()
            }
        }
    }
}

/// `@hono/hono` → `@jsr/hono__hono`
pub fn jsr_wire_name(name: &str) -> String {
    let rest = name.strip_prefix('@').unwrap_or(name);
    match rest.split_once('/') {
        Some((scope, pkg)) => format!("@jsr/{scope}__{pkg}"),
        None => format!("@jsr/{rest}"),
    }
}

/// `@jsr/std__internal` → `@std/internal`
pub fn jsr_real_name(wire: &str) -> String {
    let rest = match wire.strip_prefix("@jsr/") {
        Some(r) => r,
        None => return wire.to_string(),
    };
    match rest.split_once("__") {
        Some((scope, pkg)) => format!("@{scope}/{pkg}"),
        None => format!("@{rest}"),
    }
}

pub fn packument_url(source: Source, name: &str) -> String {
    match source {
        Source::Npm => {
            let encoded = if let Some(rest) = name.strip_prefix('@') {
                match rest.split_once('/') {
                    Some((scope, pkg)) => format!("@{scope}%2f{pkg}"),
                    None => name.to_string(),
                }
            } else {
                name.to_string()
            };
            format!("https://registry.npmjs.org/{encoded}")
        }
        Source::Jsr => format!("https://npm.jsr.io/{}", jsr_wire_name(name)),
    }
}

pub fn fetch_packument(
    client: &Client,
    source: Source,
    name: &str,
) -> Result<Packument, RegistryError> {
    let url = packument_url(source, name);
    let json = client.get_json(&url).map_err(|e| match e {
        HttpError::Status { code: 404, .. } => RegistryError::NotFound(name.to_string()),
        other => RegistryError::Network(other),
    })?;

    let versions_obj = json
        .get("versions")
        .and_then(|v| v.as_obj())
        .ok_or_else(|| RegistryError::BadResponse("missing 'versions'".into()))?;

    let mut versions = Vec::with_capacity(versions_obj.len());
    for (version_text, meta) in versions_obj {
        let version = match Version::parse(version_text) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let tarball = match meta.str_path(&["dist", "tarball"]) {
            Some(t) => t.to_string(),
            None => continue,
        };
        let optional = dep_refs(meta, "optionalDependencies");
        // npm treats an entry listed in both maps as optional.
        let mut deps = dep_refs(meta, "dependencies");
        deps.retain(|d| !optional.iter().any(|o| o.name == d.name));
        versions.push(VersionMeta {
            version,
            tarball,
            integrity: meta.str_path(&["dist", "integrity"]).map(|s| s.to_string()),
            deps,
            optional,
            peer: peer_refs(meta),
            os: string_list(meta.get("os")),
            cpu: string_list(meta.get("cpu")),
            libc: string_list(meta.get("libc")),
        });
    }

    let dist_tags = json
        .get("dist-tags")
        .and_then(|d| d.as_obj())
        .map(|tags| {
            tags.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    Ok(Packument {
        dist_tags,
        versions,
    })
}

/// Picks the version to install for a requirement.
///
/// `None` means "latest". A requirement may be a dist-tag, an exact version
/// or any range semver understands.
pub fn resolve<'a>(
    packument: &'a Packument,
    requirement: Option<&str>,
) -> Result<&'a VersionMeta, ResolveError> {
    let latest_text = packument
        .dist_tags
        .iter()
        .find(|(k, _)| k == "latest")
        .map(|(_, v)| v.clone())
        .or_else(|| packument.latest().map(|m| m.version.to_string()));

    let requirement = match requirement {
        None => {
            return packument.latest().ok_or(ResolveError::NoMatch {
                range: "*".into(),
                latest: latest_text,
            })
        }
        Some(r) if r.is_empty() || r == "*" || r == "latest" => {
            return packument.latest().ok_or(ResolveError::NoMatch {
                range: "*".into(),
                latest: latest_text,
            })
        }
        Some(r) => r.trim(),
    };

    // dist-tag such as "next" or "beta"
    if let Some(meta) = packument.tag(requirement) {
        return Ok(meta);
    }

    // exact version that may not exist yet
    if let Ok(exact) = Version::parse(requirement) {
        if let Some(meta) = packument.version(&exact) {
            return Ok(meta);
        }
        return Err(ResolveError::VersionMissing {
            requested: requirement.to_string(),
            latest: latest_text,
        });
    }

    let range = Range::parse(requirement).map_err(|_| ResolveError::NoMatch {
        range: requirement.to_string(),
        latest: latest_text.clone(),
    })?;
    packument
        .max_satisfying(&range)
        .ok_or(ResolveError::NoMatch {
            range: requirement.to_string(),
            latest: latest_text,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_jsr_names_both_ways() {
        assert_eq!(jsr_wire_name("@hono/hono"), "@jsr/hono__hono");
        assert_eq!(jsr_real_name("@jsr/std__internal"), "@std/internal");
        assert_eq!(jsr_real_name("left-pad"), "left-pad");
    }

    #[test]
    fn builds_registry_urls() {
        assert_eq!(
            packument_url(Source::Npm, "@next/env"),
            "https://registry.npmjs.org/@next%2fenv"
        );
        assert_eq!(
            packument_url(Source::Npm, "axios"),
            "https://registry.npmjs.org/axios"
        );
        assert_eq!(
            packument_url(Source::Jsr, "@hono/hono"),
            "https://npm.jsr.io/@jsr/hono__hono"
        );
    }

    fn meta(v: &str, tarball: &str) -> VersionMeta {
        VersionMeta {
            version: Version::parse(v).unwrap(),
            tarball: tarball.into(),
            integrity: None,
            deps: Vec::new(),
            optional: Vec::new(),
            peer: Vec::new(),
            os: Vec::new(),
            cpu: Vec::new(),
            libc: Vec::new(),
        }
    }

    #[test]
    fn platform_guards_follow_npm_rules() {
        let mut m = meta("1.0.0", "t");
        let linux_x64 = Platform {
            os: "linux".into(),
            cpu: "x64".into(),
            libc: Some("glibc".into()),
        };
        assert!(supports(&m, &linux_x64));

        m.os = vec!["darwin".into()];
        assert!(!supports(&m, &linux_x64));

        m.os = vec!["linux".into()];
        m.cpu = vec!["arm64".into()];
        assert!(!supports(&m, &linux_x64));

        m.cpu = vec!["!arm64".into()];
        assert!(supports(&m, &linux_x64));

        m.cpu = vec![ "!x64".into()];
        assert!(!supports(&m, &linux_x64));

        m.cpu = vec![];
        m.libc = vec!["musl".into()];
        assert!(!supports(&m, &linux_x64));
    }

    #[test]
    fn parses_optional_peer_and_platform_fields() {
        let json = crate::json::Json::parse(
            r#"{"versions":{"1.0.0":{
                "dist":{"tarball":"https://x/e.tgz","integrity":"sha512-aa"},
                "dependencies":{"core":"^1.0.0","@jsr/std__path":"1.0.0"},
                "optionalDependencies":{"@esbuild/linux-x64":"0.25.0","core":"^2"},
                "peerDependencies":{"react":">=18","sharp":"*"},
                "peerDependenciesMeta":{"sharp":{"optional":true}},
                "os":["linux"],"cpu":["x64"],"libc":["glibc"]
            }}}"#,
        )
        .unwrap();
        let versions = json.get("versions").and_then(|v| v.as_obj()).unwrap();
        let meta_json = &versions[0].1;

        let optional = dep_refs(meta_json, "optionalDependencies");
        assert_eq!(optional.len(), 2);
        assert_eq!(optional[0].name, "@esbuild/linux-x64");

        let peers = peer_refs(meta_json);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].name, "react");

        assert_eq!(string_list(meta_json.get("os")), vec!["linux".to_string()]);
        assert_eq!(string_list(meta_json.get("libc")), vec!["glibc".to_string()]);
    }

    #[test]
    fn reads_jsr_dependencies_from_the_compat_endpoint() {
        let json = crate::json::Json::parse(
            r#"{"versions":{"1.0.0":{"dist":{"tarball":"t"},"dependencies":{"@jsr/std__internal":"1.0.0"}}}}"#,
        )
        .unwrap();
        let versions = json.get("versions").and_then(|v| v.as_obj()).unwrap();
        let deps = dep_refs(&versions[0].1, "dependencies");
        assert_eq!(deps[0].name, "@std/internal");
        assert_eq!(deps[0].source, Source::Jsr);
    }

    fn packument() -> Packument {
        Packument {
            dist_tags: vec![("latest".into(), "1.4.0".into())],
            versions: vec![
                meta("1.0.0", "t1"),
                meta("1.4.0", "t2"),
                meta("2.0.0", "t3"),
                meta("2.1.0-beta.1", "t4"),
            ],
        }
    }

    #[test]
    fn resolves_ranges_and_tags() {
        let p = packument();
        assert_eq!(resolve(&p, None).unwrap().tarball, "t2");
        assert_eq!(resolve(&p, Some("^1.0.0")).unwrap().tarball, "t2");
        assert_eq!(resolve(&p, Some("2.0.0")).unwrap().tarball, "t3");
        assert_eq!(resolve(&p, Some("latest")).unwrap().tarball, "t2");
        assert_eq!(resolve(&p, Some("1.0.0")).unwrap().tarball, "t1");
    }

    #[test]
    fn reports_missing_versions() {
        let p = packument();
        match resolve(&p, Some("3.0.0")) {
            Err(ResolveError::VersionMissing { latest, .. }) => {
                assert_eq!(latest.as_deref(), Some("1.4.0"))
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
