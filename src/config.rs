//! Store layout.
//!
//! The spec puts the store at `/local/` on the machine:
//!
//! ```text
//! /local/
//!   nothingdatabase/nothingda.db
//!   libraries/axios/2.8.6.zip
//!   libraries/axios/2.3.4.zip
//!   libraries/@next/server/14.0.0.zip
//! ```
//!
//! Every version gets its own zip, so two projects needing different
//! versions of one package never fight over a single file. 0.1.x stored
//! `libraries/axios.zip`; those files are migrated on first run.
//!
//! We honour that literally when `/local` is writable, and otherwise fall
//! back to `$HOME/local` (or `$NTH_STORE` when set) so nothing works without
//! root.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn discover() -> Result<Store, String> {
        if let Ok(custom) = std::env::var("NTH_STORE") {
            if !custom.trim().is_empty() {
                return Ok(Store {
                    root: PathBuf::from(custom),
                });
            }
        }

        let system = PathBuf::from("/local");
        if system.is_dir() && dir_writable(&system) {
            return Ok(Store { root: system });
        }
        if !system.exists() && fs::create_dir_all(&system).is_ok() && dir_writable(&system) {
            return Ok(Store { root: system });
        }

        let home = std::env::var("HOME")
            .map_err(|_| "cannot locate store: neither /local nor $HOME is usable".to_string())?;
        Ok(Store {
            root: PathBuf::from(home).join("local"),
        })
    }

    pub fn ensure(&self) -> io::Result<()> {
        fs::create_dir_all(self.db_dir())?;
        fs::create_dir_all(self.libraries())?;
        Ok(())
    }

    pub fn db_dir(&self) -> PathBuf {
        self.root.join("nothingdatabase")
    }

    pub fn db_path(&self) -> PathBuf {
        self.db_dir().join("nothingda.db")
    }

    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }

    /// `libraries/axios/2.8.6.zip` or `libraries/@next/server/14.0.0.zip`.
    pub fn zip_path(&self, name: &str, version: &str) -> PathBuf {
        self.libraries().join(versioned_rel_path(name, version))
    }

    /// The folder holding every version of one package.
    pub fn package_dir(&self, name: &str) -> PathBuf {
        self.libraries().join(package_rel_dir(name))
    }

    /// Pre-0.2 layout: `libraries/axios.zip`.
    pub fn legacy_zip_path(&self, name: &str) -> PathBuf {
        self.libraries().join(zip_rel_path(name))
    }

}

/// `axios` → `axios`, `@next/server` → `@next/server`.
pub fn package_rel_dir(name: &str) -> PathBuf {
    let mut path = PathBuf::new();
    if let Some(rest) = name.strip_prefix('@') {
        if let Some((scope, pkg)) = rest.split_once('/') {
            path.push(format!("@{scope}"));
            path.push(pkg);
            return path;
        }
    }
    path.push(name);
    path
}

/// `axios` + `2.8.6` → `axios/2.8.6.zip`

/// `axios` → `axios.zip`, `@next/server` → `@next/server.zip`.
pub fn versioned_rel_path(name: &str, version: &str) -> PathBuf {
    package_rel_dir(name).join(format!("{version}.zip"))
}

/// Pre-0.2 layout: `axios.zip`, `@next/server.zip`.
pub fn zip_rel_path(name: &str) -> PathBuf {
    let mut path = PathBuf::new();
    if let Some(rest) = name.strip_prefix('@') {
        if let Some((scope, pkg)) = rest.split_once('/') {
            path.push(format!("@{scope}"));
            path.push(format!("{pkg}.zip"));
            return path;
        }
    }
    path.push(format!("{name}.zip"));
    path
}

fn dir_writable(path: &Path) -> bool {
    let probe = path.join(".nth-write-test");
    match fs::write(&probe, b"") {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_names_get_a_scope_folder() {
        assert_eq!(zip_rel_path("axios"), PathBuf::from("axios.zip"));
        assert_eq!(
            zip_rel_path("@next/server"),
            PathBuf::from("@next").join("server.zip")
        );
    }

    #[test]
    fn every_version_gets_its_own_zip() {
        assert_eq!(
            versioned_rel_path("axios", "2.8.6"),
            PathBuf::from("axios").join("2.8.6.zip")
        );
        assert_eq!(
            versioned_rel_path("@next/server", "14.0.0"),
            PathBuf::from("@next").join("server").join("14.0.0.zip")
        );
    }
}
