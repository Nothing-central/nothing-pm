//! Zip reading and writing, from scratch.
//!
//! Every package in the store is one zip file holding pure upstream code.
//! Entries are written uncompressed (method 0) — the payload is already a
//! tarball of source files, so recompressing would cost CPU without saving
//! much, and the format stays trivially readable. The reader still handles
//! deflated entries so any zip we write or find can be extracted.

use crate::hash;
use crate::inflate;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub struct ZipEntry {
    pub path: String,
    pub data: Vec<u8>,
    /// Unix permission bits, e.g. 0o644 or 0o755.
    pub mode: u32,
}

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;

/// Takes ownership so a package's file data is moved into the archive
/// instead of copied — a 150mb tarball would otherwise double in memory.
pub fn write_zip(entries: Vec<ZipEntry>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();

    for entry in &entries {
        let name = entry.path.as_bytes();
        let crc = hash::crc32(&entry.data);
        let size = entry.data.len() as u32;
        let offset = out.len() as u32;

        // local file header
        out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes()); // compressed
        out.extend_from_slice(&size.to_le_bytes()); // uncompressed
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name);
        out.extend_from_slice(&entry.data);

        // central directory record
        central.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // method
        central.extend_from_slice(&0u16.to_le_bytes()); // time
        central.extend_from_slice(&0u16.to_le_bytes()); // date
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&((entry.mode & 0o777) << 16).to_le_bytes()); // external
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }

    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&EOCD_SIG.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk
    out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len
    out
}

pub fn read_zip(bytes: &[u8]) -> Result<Vec<ZipEntry>, String> {
    let eocd = find_eocd(bytes).ok_or("not a zip archive (no end-of-central-directory)")?;
    let count = read_u16(bytes, eocd + 10)? as usize;
    let mut pos = read_u32(bytes, eocd + 16)? as usize;
    let mut entries = Vec::with_capacity(count);

    for _ in 0..count {
        if read_u32(bytes, pos)? != CENTRAL_SIG {
            return Err("corrupt zip central directory".into());
        }
        let flags = read_u16(bytes, pos + 8)?;
        let method = read_u16(bytes, pos + 10)?;
        let comp_size = read_u32(bytes, pos + 20)? as usize;
        let name_len = read_u16(bytes, pos + 28)? as usize;
        let extra_len = read_u16(bytes, pos + 30)? as usize;
        let comment_len = read_u16(bytes, pos + 32)? as usize;
        let external = read_u32(bytes, pos + 38)?;
        let local_off = read_u32(bytes, pos + 42)? as usize;

        let name_bytes = bytes
            .get(pos + 46..pos + 46 + name_len)
            .ok_or("truncated zip entry name")?;
        let name = if flags & 0x0800 != 0 {
            String::from_utf8_lossy(name_bytes).to_string()
        } else {
            // non-UTF8 zips from old tools: latin-1 fallback
            name_bytes.iter().map(|b| *b as char).collect()
        };

        // Local header carries its own name/extra lengths.
        if read_u32(bytes, local_off)? != LOCAL_SIG {
            return Err(format!("corrupt zip local header for '{name}'"));
        }
        let local_name_len = read_u16(bytes, local_off + 26)? as usize;
        let local_extra_len = read_u16(bytes, local_off + 28)? as usize;
        let data_start = local_off + 30 + local_name_len + local_extra_len;
        let data_end = data_start
            .checked_add(comp_size)
            .ok_or("zip entry size overflow")?;
        let raw = bytes
            .get(data_start..data_end)
            .ok_or("truncated zip entry data")?;

        let data = match method {
            0 => raw.to_vec(),
            8 => inflate::inflate_raw(raw).map_err(|e| format!("inflate '{name}': {e}"))?,
            other => return Err(format!("unsupported zip method {other} for '{name}'")),
        };

        let mode = (external >> 16) & 0o777;
        entries.push(ZipEntry {
            path: name,
            data,
            mode: if mode == 0 { 0o644 } else { mode },
        });

        pos += 46 + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

fn find_eocd(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 22 {
        return None;
    }
    let lowest = bytes.len().saturating_sub(22 + 65_535);
    let mut i = bytes.len() - 22;
    loop {
        if read_u32(bytes, i).ok() == Some(EOCD_SIG) {
            return Some(i);
        }
        if i == lowest {
            return None;
        }
        i -= 1;
    }
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16, String> {
    let slice = bytes.get(at..at + 2).ok_or("truncated zip")?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, String> {
    let slice = bytes.get(at..at + 4).ok_or("truncated zip")?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Extract every entry under `dest`, preserving relative paths.
pub fn extract_to(bytes: &[u8], dest: &Path) -> io::Result<()> {
    let entries = read_zip(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    for entry in entries {
        // Refuse absolute paths and traversal out of the destination.
        let rel = entry.path.trim_start_matches('/');
        let mut target = dest.to_path_buf();
        for part in rel.split('/') {
            match part {
                "" | "." => continue,
                ".." => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unsafe path in archive: {}", entry.path),
                    ))
                }
                segment => target.push(segment),
            }
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::File::create(&target)?;
        f.write_all(&entry.data)?;
        drop(f);
        set_mode(&target, entry.mode)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if mode & 0o111 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_archives() {
        let entries = vec![
            ZipEntry {
                path: "package.json".into(),
                data: br#"{"name":"x"}"#.to_vec(),
                mode: 0o644,
            },
            ZipEntry {
                path: "lib/index.js".into(),
                data: b"module.exports = 1;".to_vec(),
                mode: 0o644,
            },
            ZipEntry {
                path: "bin/cli.js".into(),
                data: b"#!/usr/bin/env node".to_vec(),
                mode: 0o755,
            },
        ];
        let zip = write_zip(entries);
        let back = read_zip(&zip).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[1].path, "lib/index.js");
        assert_eq!(back[1].data, b"module.exports = 1;");
        assert_eq!(back[2].mode, 0o755);
    }
}
