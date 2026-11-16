//! Turning a registry tarball into a store zip, and moving zips around.
//!
//! Store layout (spec §1): one zip per package *version*, under a folder
//! named after the package — `libraries/axios/2.8.6.zip`. Scoped packages
//! get a scope folder: `libraries/@next/server/14.0.0.zip`. A zip contains
//! pure upstream code: the tarball's single top-level `package/` directory
//! is stripped so `node_modules/axios/index.js` lands where Node expects it.

use crate::config::Store;
use crate::db::Db;
use crate::hash;
use crate::tar;
use crate::zipstore::{self, ZipEntry};
use std::fs;
use std::io;
use std::path::Path;

pub struct BuildResult {
    pub zip: Vec<u8>,
    pub checksum: String,
    pub size_kb: u64,
}

/// gunzip → tar → strip root dir → zip
pub fn tarball_to_zip(tarball: &[u8]) -> Result<BuildResult, String> {
    let tar_bytes = crate::inflate::gunzip(tarball)?;
    let entries = tar::parse(&tar_bytes)?;
    if entries.is_empty() {
        return Err("tarball contains no files".into());
    }

    let root = common_root(&entries);
    let mut zip_entries = Vec::new();
    for entry in &entries {
        if entry.is_dir {
            continue;
        }
        let path = match &root {
            Some(prefix) => match entry.path.strip_prefix(prefix.as_str()) {
                Some(rest) => rest.to_string(),
                None => entry.path.clone(),
            },
            None => entry.path.clone(),
        };
        if path.is_empty() {
            continue;
        }
        let mode = if entry.mode & 0o111 != 0 { 0o755 } else { 0o644 };
        zip_entries.push(ZipEntry {
            path,
            data: entry.data.clone(),
            mode,
        });
    }
    if zip_entries.is_empty() {
        return Err("tarball contains no regular files".into());
    }

    let zip = zipstore::write_zip(&zip_entries);
    let checksum = format!("sha256:{}", hash::sha256_hex(&zip));
    let size_kb = ((zip.len() as u64) / 1024).max(1);
    Ok(BuildResult {
        zip,
        checksum,
        size_kb,
    })
}

/// npm and jsr tarballs both wrap everything in `package/`, but do not rely
/// on that: use the single shared leading directory if there is one.
fn common_root(entries: &[tar::Entry]) -> Option<String> {
    let mut root: Option<String> = None;
    for entry in entries {
        let first = entry.path.split('/').next()?.to_string();
        if entry.path.contains('/') {
            match &root {
                None => root = Some(first),
                Some(existing) if existing == &first => {}
                Some(_) => return None,
            }
        } else {
            return None; // a file at the archive root: no stripping
        }
    }
    // Keep the separator so `package/package.json` strips to `package.json`.
    root.map(|r| format!("{r}/"))
}

pub fn write_version(store: &Store, name: &str, version: &str, zip: &[u8]) -> io::Result<()> {
    let path = store.zip_path(name, version);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(&path, zip)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("zip.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

pub fn read_version(store: &Store, name: &str, version: &str) -> io::Result<Option<Vec<u8>>> {
    read_optional(&store.zip_path(name, version))
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn remove_version(store: &Store, name: &str, version: &str) -> io::Result<bool> {
    remove_if_exists(&store.zip_path(name, version))
}

/// Remove every stored version of a package, including any pre-0.2 zip, and
/// the by-now empty folder it lived in.
pub fn remove_package(store: &Store, name: &str) -> io::Result<bool> {
    let dir = store.package_dir(name);
    let mut removed = remove_if_exists(&store.legacy_zip_path(name))?;
    if dir.is_dir() {
        fs::remove_dir_all(&dir)?;
        removed = true;
        // `libraries/@next` should not linger once its last package is gone.
        if let Some(scope) = dir.parent() {
            if scope != store.libraries()
                && fs::read_dir(scope)
                    .map(|mut entries| entries.next().is_none())
                    .unwrap_or(false)
            {
                let _ = fs::remove_dir(scope);
            }
        }
    }
    Ok(removed)
}

fn remove_if_exists(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// 0.1.x kept one `libraries/<name>.zip` per package. Move each one to the
/// versioned path, using the version recorded in the db. Returns how many
/// files were moved.
pub fn migrate_legacy(store: &Store, db: &Db) -> io::Result<usize> {
    let mut moved = 0;
    for pkg in db.all().unwrap_or_default() {
        let legacy = store.legacy_zip_path(&pkg.name);
        let target = store.zip_path(&pkg.name, &pkg.version);
        if target.is_file() || !legacy.is_file() {
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::rename(&legacy, &target) {
            Ok(()) => moved += 1,
            Err(_) => {
                // Cross-device or a leftover: copy then drop the original.
                if fs::copy(&legacy, &target).is_ok() {
                    let _ = fs::remove_file(&legacy);
                    moved += 1;
                }
            }
        }
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_package_root() {
        let tar_bytes = build_tar(&[
            ("package/package.json", br#"{"name":"x","version":"1.0.0"}"#.as_ref()),
            ("package/lib/index.js", b"module.exports = 1;".as_ref()),
        ]);
        let gz = crate::testutil::gzip_store_only(&tar_bytes);
        let built = tarball_to_zip(&gz).unwrap();
        let entries = zipstore::read_zip(&built.zip).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["package.json", "lib/index.js"]);
        assert!(built.checksum.starts_with("sha256:"));
    }

    fn build_tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (path, data) in files {
            let mut header = [0u8; 512];
            header[..path.len()].copy_from_slice(path.as_bytes());
            header[100..108].copy_from_slice(b"0000644\0");
            header[108..116].copy_from_slice(b"0000000\0");
            header[116..124].copy_from_slice(b"0000000\0");
            let size = format!("{:011o}\0", data.len());
            header[124..136].copy_from_slice(size.as_bytes());
            header[136..148].copy_from_slice(b"00000000000\0");
            header[156] = b'0';
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");
            let mut sum: u32 = header.iter().map(|b| *b as u32).sum();
            sum += 8 * b' ' as u32;
            let chk = format!("{:06o}\0 ", sum);
            header[148..156].copy_from_slice(chk.as_bytes());
            out.extend_from_slice(&header);
            out.extend_from_slice(data);
            let pad = (512 - data.len() % 512) % 512;
            out.extend(std::iter::repeat(0u8).take(pad));
        }
        out.extend(std::iter::repeat(0u8).take(1024));
        out
    }
}
