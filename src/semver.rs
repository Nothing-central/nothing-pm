//! A from-scratch semver implementation modelled on node-semver.
//!
//! Covers everything nothing needs: exact versions, caret, tilde, x-ranges,
//! star, hyphen ranges, comparator sets joined by whitespace (AND) and `||`
//! (OR), prerelease identifiers, and node-semver's rule that a prerelease
//! version only satisfies a range when some comparator on the same
//! major.minor.patch tuple itself carries a prerelease.

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PreIdent {
    Num(u64),
    Str(String),
}

// Derived ordering puts Num before Str, which is exactly what semver wants.

impl fmt::Display for PreIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PreIdent::Num(n) => write!(f, "{n}"),
            PreIdent::Str(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<PreIdent>,
    pub build: String,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version {
            major,
            minor,
            patch,
            pre: Vec::new(),
            build: String::new(),
        }
    }

    /// `2.0.0-0`: the lowest possible prerelease of a version. node-semver
    /// uses it as an exclusive upper bound that also excludes prereleases.
    pub fn pre_zero(major: u64, minor: u64, patch: u64) -> Version {
        Version {
            major,
            minor,
            patch,
            pre: vec![PreIdent::Num(0)],
            build: String::new(),
        }
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    pub fn parse(s: &str) -> Result<Version, String> {
        let text = s.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.strip_prefix('=').unwrap_or(text);
        let (without_build, build) = match text.find('+') {
            Some(i) => (&text[..i], &text[i + 1..]),
            None => (text, ""),
        };
        let (core, pre_str) = match without_build.find('-') {
            Some(i) => (&without_build[..i], Some(&without_build[i + 1..])),
            None => (without_build, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 {
            return Err(format!("invalid version '{s}'"));
        }
        let major = parse_num(parts[0]).ok_or_else(|| format!("invalid version '{s}'"))?;
        let minor = parse_num(parts[1]).ok_or_else(|| format!("invalid version '{s}'"))?;
        let patch = parse_num(parts[2]).ok_or_else(|| format!("invalid version '{s}'"))?;
        let pre = match pre_str {
            Some(p) => parse_pre(p).ok_or_else(|| format!("invalid version '{s}'"))?,
            None => Vec::new(),
        };
        Ok(Version {
            major,
            minor,
            patch,
            pre,
            build: build.to_string(),
        })
    }
}

fn parse_num(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return None; // no leading zeroes
    }
    s.parse().ok()
}

fn parse_pre(s: &str) -> Option<Vec<PreIdent>> {
    if s.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for part in s.split('.') {
        if part.is_empty() {
            return None;
        }
        if part.bytes().all(|b| b.is_ascii_digit()) {
            if part.len() > 1 && part.starts_with('0') {
                return None;
            }
            out.push(PreIdent::Num(part.parse().ok()?));
        } else if part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            out.push(PreIdent::Str(part.to_string()));
        } else {
            return None;
        }
    }
    Some(out)
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let left = (self.major, self.minor, self.patch);
        let right = (other.major, other.minor, other.patch);
        match left.cmp(&right) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater, // a release outranks its prereleases
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(other.pre.iter()) {
                    match a.cmp(b) {
                        Ordering::Equal => continue,
                        ord => return ord,
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            let ids: Vec<String> = self.pre.iter().map(|p| p.to_string()).collect();
            write!(f, "-{}", ids.join("."))?;
        }
        if !self.build.is_empty() {
            write!(f, "+{}", self.build)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Lt,
    Lte,
    Gt,
    Gte,
    Eq,
}

impl Op {
    fn from_str(s: &str) -> Option<Op> {
        Some(match s {
            "<" => Op::Lt,
            "<=" => Op::Lte,
            ">" => Op::Gt,
            ">=" => Op::Gte,
            "=" | "==" => Op::Eq,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comparator {
    pub op: Op,
    pub version: Version,
}

impl Comparator {
    fn new(op: Op, version: Version) -> Comparator {
        Comparator { op, version }
    }

    fn test(&self, v: &Version) -> bool {
        match self.op {
            Op::Lt => v < &self.version,
            Op::Lte => v <= &self.version,
            Op::Gt => v > &self.version,
            Op::Gte => v >= &self.version,
            Op::Eq => v == &self.version,
        }
    }
}

/// An OR-list of AND-groups. An empty inner vector means "any version".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    pub raw: String,
    pub sets: Vec<Vec<Comparator>>,
}

impl Range {
    pub fn parse(input: &str) -> Result<Range, String> {
        let raw = input.trim().to_string();
        let mut sets = Vec::new();
        for alternative in raw.split("||") {
            sets.push(parse_set(alternative)?);
        }
        if sets.is_empty() {
            sets.push(Vec::new());
        }
        Ok(Range { raw, sets })
    }

    pub fn test(&self, v: &Version) -> bool {
        self.sets.iter().any(|set| test_set(set, v))
    }
}

fn test_set(set: &[Comparator], version: &Version) -> bool {
    for c in set {
        if !c.test(version) {
            return false;
        }
    }
    if version.is_prerelease() {
        let allowed = set.iter().any(|c| {
            c.version.is_prerelease()
                && c.version.major == version.major
                && c.version.minor == version.minor
                && c.version.patch == version.patch
        });
        if !allowed {
            return false;
        }
    }
    true
}

/// A partially specified version; `None` means an x/wildcard component.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Partial {
    major: Option<u64>,
    minor: Option<u64>,
    patch: Option<u64>,
    pre: Vec<PreIdent>,
}

impl Partial {
    fn is_x(&self) -> bool {
        self.major.is_none()
    }

    fn full(&self) -> Version {
        Version {
            major: self.major.unwrap_or(0),
            minor: self.minor.unwrap_or(0),
            patch: self.patch.unwrap_or(0),
            pre: self.pre.clone(),
            build: String::new(),
        }
    }
}

fn is_x_component(s: &str) -> bool {
    let s = s.trim();
    s.is_empty() || s == "x" || s == "X" || s == "*"
}

fn parse_partial(s: &str) -> Option<Partial> {
    let text = s.trim();
    let text = text.strip_prefix('v').unwrap_or(text);
    let without_build = match text.find('+') {
        Some(i) => &text[..i],
        None => text,
    };
    let (core, pre_str) = match without_build.find('-') {
        Some(i) => (&without_build[..i], Some(&without_build[i + 1..])),
        None => (without_build, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut comps: [Option<u64>; 3] = [None, None, None];
    let mut wildcard_seen = false;
    for (idx, part) in parts.iter().enumerate() {
        if is_x_component(part) {
            wildcard_seen = true;
            comps[idx] = None;
            continue;
        }
        if wildcard_seen {
            return None; // "1.x.3" is not a valid range
        }
        comps[idx] = Some(parse_num(part)?);
    }
    let pre = match pre_str {
        Some(p) => parse_pre(p)?,
        None => Vec::new(),
    };
    if !pre.is_empty() && comps[2].is_none() {
        return None;
    }
    Some(Partial {
        major: comps[0],
        minor: comps[1],
        patch: comps[2],
        pre,
    })
}

fn parse_set(input: &str) -> Result<Vec<Comparator>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    // npm ranges are written both as `>=1.2.3` and as `>= 1.2.3`; a bare
    // operator is joined to whatever follows it before tokenising.
    let mut tokens: Vec<String> = Vec::new();
    for raw in trimmed.split_whitespace() {
        match tokens.last_mut() {
            Some(last) if is_bare_operator(last) => last.push_str(raw),
            _ => tokens.push(raw.to_string()),
        }
    }

    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        // hyphen range: "1.2.3 - 2.3.4"
        if i + 1 < tokens.len() && tokens[i + 1] == "-" {
            if i + 2 >= tokens.len() {
                return Err(format!("dangling hyphen range in '{input}'"));
            }
            out.extend(hyphen_range(&tokens[i], &tokens[i + 2])?);
            i += 3;
            continue;
        }
        out.extend(desugar(&tokens[i])?);
        i += 1;
    }
    Ok(out)
}

fn is_bare_operator(token: &str) -> bool {
    matches!(token, ">=" | "<=" | ">" | "<" | "=" | "==" | "~" | "~>" | "^")
}

fn hyphen_range(from: &str, to: &str) -> Result<Vec<Comparator>, String> {
    let lo = parse_partial(from).ok_or_else(|| format!("invalid range '{from}'"))?;
    let hi = parse_partial(to).ok_or_else(|| format!("invalid range '{to}'"))?;
    let mut out = Vec::new();
    if let Some(major) = lo.major {
        out.push(Comparator::new(Op::Gte, lo.full()));
        let _ = major;
    }
    if let Some(major) = hi.major {
        if hi.minor.is_none() {
            out.push(Comparator::new(Op::Lt, Version::pre_zero(major + 1, 0, 0)));
        } else if hi.patch.is_none() {
            out.push(Comparator::new(
                Op::Lt,
                Version::pre_zero(major, hi.minor.unwrap() + 1, 0),
            ));
        } else {
            out.push(Comparator::new(Op::Lte, hi.full()));
        }
    }
    Ok(out)
}

fn desugar(token: &str) -> Result<Vec<Comparator>, String> {
    let t = token.trim();
    if t.is_empty() || t == "*" || t == "x" || t == "X" {
        return Ok(Vec::new());
    }
    // A dangling operator (`>=` with no version) means "any version".
    if is_bare_operator(t) {
        return Ok(Vec::new());
    }
    if let Some(rest) = t.strip_prefix('^') {
        return caret(rest).ok_or_else(|| format!("invalid range '{token}'"));
    }
    if let Some(rest) = t.strip_prefix('~') {
        let rest = rest.strip_prefix('>').unwrap_or(rest);
        return tilde(rest).ok_or_else(|| format!("invalid range '{token}'"));
    }
    let mut op: Option<Op> = None;
    let mut rest = t;
    for candidate in [">=", "<=", "==", ">", "<", "="] {
        if let Some(r) = t.strip_prefix(candidate) {
            op = Op::from_str(candidate);
            rest = r;
            break;
        }
    }
    let partial = parse_partial(rest).ok_or_else(|| format!("invalid range '{token}'"))?;
    Ok(x_range(op, partial))
}

fn caret(rest: &str) -> Option<Vec<Comparator>> {
    let p = parse_partial(rest)?;
    if p.is_x() {
        return Some(Vec::new());
    }
    let major = p.major.unwrap();
    if p.minor.is_none() {
        return Some(vec![
            Comparator::new(Op::Gte, Version::new(major, 0, 0)),
            Comparator::new(Op::Lt, Version::pre_zero(major + 1, 0, 0)),
        ]);
    }
    let minor = p.minor.unwrap();
    if p.patch.is_none() {
        let upper = if major == 0 {
            Version::pre_zero(0, minor + 1, 0)
        } else {
            Version::pre_zero(major + 1, 0, 0)
        };
        return Some(vec![
            Comparator::new(Op::Gte, Version::new(major, minor, 0)),
            Comparator::new(Op::Lt, upper),
        ]);
    }
    let patch = p.patch.unwrap();
    let upper = if major == 0 {
        if minor == 0 {
            Version::pre_zero(0, 0, patch + 1)
        } else {
            Version::pre_zero(0, minor + 1, 0)
        }
    } else {
        Version::pre_zero(major + 1, 0, 0)
    };
    Some(vec![
        Comparator::new(Op::Gte, p.full()),
        Comparator::new(Op::Lt, upper),
    ])
}

fn tilde(rest: &str) -> Option<Vec<Comparator>> {
    let p = parse_partial(rest)?;
    if p.is_x() {
        return Some(Vec::new());
    }
    let major = p.major.unwrap();
    if p.minor.is_none() {
        return Some(vec![
            Comparator::new(Op::Gte, Version::new(major, 0, 0)),
            Comparator::new(Op::Lt, Version::pre_zero(major + 1, 0, 0)),
        ]);
    }
    let minor = p.minor.unwrap();
    let lower = if p.patch.is_none() {
        Version::new(major, minor, 0)
    } else {
        p.full()
    };
    Some(vec![
        Comparator::new(Op::Gte, lower),
        Comparator::new(Op::Lt, Version::pre_zero(major, minor + 1, 0)),
    ])
}

fn x_range(op: Option<Op>, p: Partial) -> Vec<Comparator> {
    let x_major = p.major.is_none();
    let x_minor = x_major || p.minor.is_none();
    let x_patch = x_minor || p.patch.is_none();
    let any_x = x_patch;
    let mut op = op;
    if op == Some(Op::Eq) && any_x {
        op = None;
    }

    if x_major {
        // "*" matches anything; ">*"/"<*" match nothing.
        return match op {
            Some(Op::Gt) | Some(Op::Lt) => {
                vec![Comparator::new(Op::Lt, Version::pre_zero(0, 0, 0))]
            }
            _ => Vec::new(),
        };
    }

    let major = p.major.unwrap();

    if let Some(o) = op {
        if any_x {
            let mut major = major;
            let mut minor = if x_minor { 0 } else { p.minor.unwrap() };
            let mut patch = 0u64;
            let mut o = o;
            if o == Op::Gt {
                o = Op::Gte;
                if x_minor {
                    major += 1;
                    minor = 0;
                    patch = 0;
                } else {
                    minor += 1;
                    patch = 0;
                }
            } else if o == Op::Lte {
                o = Op::Lt;
                if x_minor {
                    major += 1;
                } else {
                    minor += 1;
                }
            }
            let version = if o == Op::Lt {
                Version::pre_zero(major, minor, patch)
            } else {
                Version::new(major, minor, patch)
            };
            return vec![Comparator::new(o, version)];
        }
        return vec![Comparator::new(o, p.full())];
    }

    if x_minor {
        return vec![
            Comparator::new(Op::Gte, Version::new(major, 0, 0)),
            Comparator::new(Op::Lt, Version::pre_zero(major + 1, 0, 0)),
        ];
    }
    if x_patch {
        let minor = p.minor.unwrap();
        return vec![
            Comparator::new(Op::Gte, Version::new(major, minor, 0)),
            Comparator::new(Op::Lt, Version::pre_zero(major, minor + 1, 0)),
        ];
    }
    vec![Comparator::new(Op::Eq, p.full())]
}

/// Highest version in `versions` that satisfies `range`.
pub fn max_satisfying<'a, I>(versions: I, range: &Range) -> Option<Version>
where
    I: IntoIterator<Item = &'a Version>,
{
    let mut best: Option<Version> = None;
    for v in versions {
        if range.test(v) {
            match &best {
                None => best = Some(v.clone()),
                Some(b) if v > b => best = Some(v.clone()),
                _ => {}
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn sat(range: &str, version: &str) -> bool {
        Range::parse(range).unwrap().test(&v(version))
    }

    #[test]
    fn ordering() {
        assert!(v("1.0.0") < v("1.0.1"));
        assert!(v("1.0.0-alpha") < v("1.0.0"));
        assert!(v("1.0.0-alpha.1") < v("1.0.0-alpha.2"));
        assert!(v("1.0.0-alpha.2") < v("1.0.0-beta"));
        assert!(v("1.0.0-1") < v("1.0.0-alpha"));
        assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), Ordering::Equal);
    }

    #[test]
    fn exact_and_caret() {
        assert!(sat("2.2.3", "2.2.3"));
        assert!(!sat("2.2.3", "2.2.4"));
        assert!(sat("^2.2.3", "2.9.9"));
        assert!(!sat("^2.2.3", "3.0.0"));
        assert!(!sat("^2.2.3", "2.2.2"));
        assert!(sat("^0.2.3", "0.2.9"));
        assert!(!sat("^0.2.3", "0.3.0"));
        assert!(sat("^0.0.3", "0.0.3"));
        assert!(!sat("^0.0.3", "0.0.4"));
        assert!(sat("^0.0.x", "0.0.7"));
        assert!(!sat("^0.0.x", "0.1.0"));
        assert!(sat("^0.x", "0.9.0"));
        assert!(!sat("^0.x", "1.0.0"));
    }

    #[test]
    fn tilde_and_xranges() {
        assert!(sat("~1.2.3", "1.2.9"));
        assert!(!sat("~1.2.3", "1.3.0"));
        assert!(sat("~1", "1.9.9"));
        assert!(sat("~0.0.1", "0.0.9"));
        assert!(!sat("~0.0.1", "0.1.0"));
        assert!(sat("1.x", "1.4.2"));
        assert!(!sat("1.x", "2.0.0"));
        assert!(sat("1.2.x", "1.2.0"));
        assert!(sat("*", "9.9.9"));
        assert!(!sat("*", "1.0.0-beta"));
    }

    #[test]
    fn tolerates_space_after_operator() {
        // npm packages really publish ranges like this (e.g. safer-buffer).
        assert!(sat(">= 2.1.2 < 3.0.0", "2.1.2"));
        assert!(!sat(">= 2.1.2 < 3.0.0", "2.1.1"));
        assert!(!sat(">= 2.1.2 < 3.0.0", "3.0.0"));
        assert!(sat("^ 1.2.3", "1.9.0"));
        assert!(sat(">= 1.0.0", "2.0.0"));
    }

    #[test]
    fn comparator_sets_and_or() {
        assert!(sat(">=1.2.7 <1.3.0", "1.2.9"));
        assert!(!sat(">=1.2.7 <1.3.0", "1.3.0"));
        assert!(sat(">=1.2.7 || <1.0.0", "0.5.0"));
        assert!(sat("1.2.3 - 2.3.4", "2.0.0"));
        assert!(!sat("1.2.3 - 2.3.4", "2.3.5"));
        assert!(sat(">1.2", "1.3.0"));
        assert!(!sat(">1.2", "1.2.9"));
        assert!(sat("<=1.2", "1.2.9"));
    }

    #[test]
    fn prerelease_gating() {
        assert!(!sat("^1.2.3", "1.3.0-beta.1"));
        assert!(sat("^1.2.3-beta.1", "1.2.3-beta.2"));
        assert!(!sat("^1.2.3-beta.1", "1.2.4-beta.1"));
    }

    #[test]
    fn picks_max_satisfying() {
        let vs = vec![v("1.0.0"), v("1.4.0"), v("2.0.0")];
        let got = max_satisfying(vs.iter(), &Range::parse("^1.0.0").unwrap());
        assert_eq!(got, Some(v("1.4.0")));
    }
}
