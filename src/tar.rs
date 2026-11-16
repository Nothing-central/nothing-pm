//! Minimal tar reader (ustar + GNU long names + pax path overrides).
//!
//! Registry tarballs are always tar inside gzip; we only need to walk
//! regular files and skip the rest.

pub struct Entry {
    /// Path relative to the tarball root, e.g. `package/lib/index.js`.
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
    /// True for directories (`typeflag == '5'`).
    pub is_dir: bool,
}

pub fn parse(data: &[u8]) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let mut pos = 0usize;
    let mut long_name: Option<String> = None;
    let mut pax_path: Option<String> = None;

    while pos + 512 <= data.len() {
        let header = &data[pos..pos + 512];
        if header.iter().all(|b| *b == 0) {
            break; // end-of-archive marker
        }
        let size = parse_octal(&header[124..136])? as usize;
        let mode = parse_octal(&header[100..108]).unwrap_or(0o644) as u32;
        let typeflag = header[156];
        let name = read_str(&header[0..100]);
        let prefix = read_str(&header[345..500]);
        let mut full = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };

        let data_start = pos + 512;
        let data_end = data_start
            .checked_add(size)
            .ok_or_else(|| "tar entry size overflow".to_string())?;
        if data_end > data.len() {
            return Err("truncated tar archive".into());
        }
        let body = &data[data_start..data_end];

        match typeflag {
            b'L' => {
                // GNU long name: body is the next entry's path
                long_name = Some(trim_nul(body));
            }
            b'x' => {
                // pax extended header: "len key=value\n" records
                if let Some(p) = pax_path_of(body) {
                    pax_path = Some(p);
                }
            }
            b'g' => {}
            b'5' => {
                if let Some(p) = pax_path.take().or_else(|| long_name.take()) {
                    full = p;
                }
                entries.push(Entry {
                    path: normalise(&full),
                    data: Vec::new(),
                    mode,
                    is_dir: true,
                });
            }
            b'0' | 0 | b'7' => {
                if let Some(p) = pax_path.take().or_else(|| long_name.take()) {
                    full = p;
                }
                entries.push(Entry {
                    path: normalise(&full),
                    data: body.to_vec(),
                    mode,
                    is_dir: false,
                });
            }
            _ => {
                // symlinks, devices, etc. are not part of a published package
                pax_path = None;
                long_name = None;
            }
        }

        // Entries are padded to 512 byte boundaries.
        pos = data_start + size.div_ceil(512) * 512;
    }
    Ok(entries)
}

fn normalise(path: &str) -> String {
    let stripped = path.trim_start_matches("./");
    stripped.trim_start_matches('/').to_string()
}

fn read_str(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

fn trim_nul(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

fn parse_octal(bytes: &[u8]) -> Result<u64, String> {
    if bytes.first() == Some(&0x80) {
        // GNU base-256 encoding
        let mut value: u64 = 0;
        for b in &bytes[1..] {
            value = (value << 8) | *b as u64;
        }
        return Ok(value);
    }
    let text = read_str(bytes);
    let text = text.trim();
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(|_| format!("invalid tar octal '{text}'"))
}

fn pax_path_of(body: &[u8]) -> Option<String> {
    for record in body.split(|b| *b == b'\n') {
        if let Some(space) = record.iter().position(|b| *b == b' ') {
            let kv = &record[space + 1..];
            if let Some(eq) = kv.iter().position(|b| *b == b'=') {
                let key = &kv[..eq];
                if key == b"path" {
                    return Some(String::from_utf8_lossy(&kv[eq + 1..]).to_string());
                }
            }
        }
    }
    None
}
