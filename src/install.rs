//! node_modules materialisation.
//!
//! The store holds zips; installing means extracting one into
//! `node_modules/<name>` (or `node_modules/@scope/<name>` for scoped
//! packages). Layout is plain Node/Bun/Deno-compatible — nothing hidden, no
//! symlink farm, no per-project store.

use crate::config::Store;
use crate::zipstore;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn node_modules(project: &Path) -> PathBuf {
    project.join("node_modules")
}

pub fn package_dir(node_modules: &Path, name: &str) -> PathBuf {
    if let Some(rest) = name.strip_prefix('@') {
        if let Some((scope, pkg)) = rest.split_once('/') {
            return node_modules.join(format!("@{scope}")).join(pkg);
        }
    }
    node_modules.join(name)
}

/// Extract one stored version of `name` into `node_modules`, replacing
/// whatever was there before.
pub fn install_from_store(
    store: &Store,
    name: &str,
    version: &str,
    node_modules: &Path,
) -> io::Result<()> {
    let bytes = crate::store::read_version(store, name, version)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{name} {version} is not in the store"),
        )
    })?;
    install_bytes(name, &bytes, node_modules)
}

pub fn install_bytes(name: &str, zip: &[u8], node_modules: &Path) -> io::Result<()> {
    let dest = package_dir(node_modules, name);
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::create_dir_all(&dest)?;
    zipstore::extract_to(zip, &dest)
}
