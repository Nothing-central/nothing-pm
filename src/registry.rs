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
        let mut deps = Vec::new();
        if let Some(deps_obj) = meta.get("dependencies").and_then(|d| d.as_obj()) {
            for (dep_name, dep_range) in deps_obj {
                let dep_range = dep_range.as_str().unwrap_or("*");
                let (name, source) = match dep_name.strip_prefix("@jsr/") {
                    Some(_) => (jsr_real_name(dep_name), Source::Jsr),
                    None => (dep_name.clone(), Source::Npm),
                };
                deps.push(DepRef {
                    name,
                    range: dep_range.to_string(),
                    source,
                });
            }
        }
        versions.push(VersionMeta {
            version,
            tarball,
            integrity: meta.str_path(&["dist", "integrity"]).map(|s| s.to_string()),
            deps,
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
        }
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
