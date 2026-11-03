//! Read only the live files in a Chromium LevelDB. Never acquire LOCK, recover
//! the database, or carve obsolete/deleted records from leftover tables.
use anyhow::{bail, ensure, Context, Result};
use leveldb_core::Record;
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_BYTES: usize = 64 * 1024 * 1024;

pub fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "Expected a regular store file"
    );
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_BYTES, "Store file exceeds size limit");
    Ok(bytes)
}

pub struct Cursor<'a>(pub &'a [u8]);
impl<'a> Cursor<'a> {
    pub fn varint(&mut self) -> Result<u64> {
        let mut value = 0;
        for shift in (0..70).step_by(7) {
            let byte = self.take(1)?[0];
            ensure!(shift < 63 || byte <= 1, "Invalid store varint");
            value |= u64::from(byte & 127) << shift;
            if byte & 128 == 0 {
                return Ok(value);
            }
        }
        bail!("Invalid store varint")
    }
    pub fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        ensure!(len <= self.0.len(), "Truncated store value");
        let (head, tail) = self.0.split_at(len);
        self.0 = tail;
        Ok(head)
    }
    fn string(&mut self) -> Result<&'a [u8]> {
        let len = usize::try_from(self.varint()?)?;
        self.take(len)
    }
}

// https://github.com/google/leveldb/blob/main/doc/log_format.md
// An incomplete trailing record is expected while the writer is appending.
fn manifest_records(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut records = vec![];
    let mut fragments: Option<Vec<u8>> = None;
    let mut pos = 0;
    while pos + 7 <= bytes.len() {
        let left = 32768 - pos % 32768;
        if left < 7 {
            pos += left;
            continue;
        }
        let header = &bytes[pos..pos + 7];
        let len = u16::from_le_bytes([header[4], header[5]]) as usize;
        let kind = header[6];
        if kind == 0 && len == 0 {
            pos += left;
            continue;
        }
        ensure!(len + 7 <= left, "Invalid manifest fragment length");
        if pos + 7 + len > bytes.len() {
            break;
        }
        let data = &bytes[pos + 7..pos + 7 + len];
        let crc = crc32c::crc32c_append(crc32c::crc32c(&[kind]), data)
            .rotate_left(17)
            .wrapping_add(0xa282_ead8);
        ensure!(
            crc == u32::from_le_bytes(header[..4].try_into()?),
            "Manifest checksum mismatch"
        );
        match kind {
            1 => {
                ensure!(fragments.is_none(), "Incomplete manifest record");
                records.push(data.to_vec());
            }
            2 => {
                ensure!(fragments.is_none(), "Incomplete manifest record");
                fragments = Some(data.to_vec());
            }
            3 | 4 => {
                let part = fragments.as_mut().context("Missing manifest fragment")?;
                ensure!(
                    part.len() + data.len() <= MAX_BYTES,
                    "Manifest record exceeds size limit"
                );
                part.extend_from_slice(data);
                if kind == 4 {
                    records.push(fragments.take().unwrap());
                }
            }
            _ => bail!("Unknown manifest fragment"),
        }
        pos += 7 + len;
    }
    Ok(records)
}

fn live_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let current = read_bounded(&dir.join("CURRENT"))?;
    let name = std::str::from_utf8(&current)?.trim();
    ensure!(
        name.starts_with("MANIFEST-")
            && name[9..].bytes().all(|b| b.is_ascii_digit())
            && name.len() > 9,
        "Invalid manifest name"
    );
    let mut tables = BTreeSet::new();
    let (mut log, mut previous_log) = (0, 0);
    // https://github.com/google/leveldb/blob/main/db/version_edit.cc
    for record in manifest_records(&read_bounded(&dir.join(name))?)? {
        let mut c = Cursor(&record);
        while !c.0.is_empty() {
            match c.varint()? {
                1 => {
                    c.string()?;
                }
                2 => log = c.varint()?,
                3 | 4 => {
                    c.varint()?;
                }
                5 => {
                    c.varint()?;
                    c.string()?;
                }
                6 => {
                    c.varint()?;
                    tables.remove(&c.varint()?);
                }
                7 => {
                    c.varint()?;
                    tables.insert(c.varint()?);
                    c.varint()?;
                    c.string()?;
                    c.string()?;
                }
                9 => previous_log = c.varint()?,
                _ => bail!("Unsupported LevelDB manifest version"),
            }
        }
    }
    let mut paths = vec![];
    for number in tables {
        let ldb = dir.join(format!("{number:06}.ldb"));
        paths.push(if ldb.exists() {
            ldb
        } else {
            dir.join(format!("{number:06}.sst"))
        });
    }
    // Recovery also includes a newly-created log not yet referenced by MANIFEST.
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("log") {
            if let Some(number) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<u64>().ok())
            {
                if number >= log || number == previous_log {
                    paths.push(path);
                }
            }
        }
    }
    ensure!(
        read_bounded(&dir.join("CURRENT"))? == current,
        "Store changed during scan; retrying"
    );
    paths.sort();
    Ok(paths)
}

pub fn read_live(dir: &Path) -> Result<HashMap<Vec<u8>, Record>> {
    ensure!(
        fs::symlink_metadata(dir)?.file_type().is_dir(),
        "Expected a store directory"
    );
    let paths = live_files(dir)?;
    let mut latest: HashMap<Vec<u8>, Record> = HashMap::new();
    let mut total = 0;
    for path in &paths {
        let bytes = read_bounded(path)?;
        total += bytes.len();
        ensure!(total <= 4 * MAX_BYTES, "Store exceeds scan size limit");
        let records = if path.extension().and_then(|s| s.to_str()) == Some("log") {
            leveldb_core::parse_log_bytes(&bytes, path)?
        } else {
            leveldb_core::parse_table_bytes(&bytes, path)?
        };
        for record in records {
            let key = record.key.clone();
            if latest.get(&key).is_none_or(|old| old.seq < record.seq) {
                latest.insert(key, record);
            }
        }
    }
    ensure!(
        paths == live_files(dir)?,
        "Store changed during scan; retrying"
    );
    latest.retain(|_, r| !r.deleted);
    Ok(latest)
}

/// Chromium KeyPrefix: encoded widths followed by little-endian IDs.
pub fn prefix(key: &[u8]) -> Result<(u64, u64, u64, &[u8])> {
    let mut c = Cursor(key);
    let size = c.take(1)?[0];
    let mut read = |len: usize| -> Result<u64> {
        let mut bytes = [0; 8];
        bytes[..len].copy_from_slice(c.take(len)?);
        Ok(u64::from_le_bytes(bytes))
    };
    let db = read((size >> 5) as usize + 1)?;
    let store = read(((size >> 2) & 7) as usize + 1)?;
    let index = read((size & 3) as usize + 1)?;
    Ok((db, store, index, c.0))
}
