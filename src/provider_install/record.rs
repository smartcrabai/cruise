use super::{Provider, pins::Pin};
use crate::error::{CruiseError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InstallRecord {
    pub schema: u32,
    pub provider: String,
    pub version: String,
    pub target: String,
    pub relative_path: String,
    pub sha256: String,
    pub len: u64,
    pub mtime_unix_nanos: u128,
    pub source_url: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub len: u64,
    pub mtime_unix_nanos: u128,
}
pub(crate) fn stamp(path: &Path) -> Result<Stamp> {
    let m = std::fs::metadata(path)?;
    if !m.is_file() {
        return Err(CruiseError::Other(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    let modified = m
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| CruiseError::Other(e.to_string()))?;
    Ok(Stamp {
        len: m.len(),
        mtime_unix_nanos: modified.as_nanos(),
    })
}
pub(crate) fn read_record(path: &Path) -> Result<InstallRecord> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| CruiseError::Other(format!("invalid install record: {e}")))
}
pub(crate) fn record_valid(
    provider: Provider,
    pin: &Pin,
    target: &str,
    dir: &Path,
    rec: &InstallRecord,
) -> bool {
    let legacy = format!("{}/{}", pin.version, provider.binary_name());
    let generated = rec.relative_path.strip_prefix(&format!("{}-", pin.version))
        .and_then(|rest| rest.strip_suffix(&format!("/{}", provider.binary_name())))
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id));
    rec.schema == 1
        && rec.provider == provider.as_str()
        && rec.version == pin.version
        && rec.target == target
        && (rec.relative_path == legacy || generated)
        && stamp(&dir.join(&rec.relative_path))
            .is_ok_and(|s| s.len == rec.len && s.mtime_unix_nanos == rec.mtime_unix_nanos)
}
pub(crate) fn sha256(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(crate::http_download::hex_digest(&hash.finalize()))
}
pub(crate) fn write_record_atomic(path: &Path, rec: &InstallRecord) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| CruiseError::Other("record has no parent".into()))?;
    let temp: PathBuf = parent.join(format!(".record-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&temp)?;
        file.write_all(&serde_json::to_vec(rec).map_err(|e| CruiseError::Other(e.to_string()))?)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(source, destination)?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // Both paths are NUL-terminated UTF-16 and remain alive for the call.
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test fixtures must succeed")]
mod tests {
    use super::*;

    #[test]
    fn atomically_replaces_an_existing_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("install.json");
        let mut record = InstallRecord {
            schema: 1,
            provider: "claude".into(),
            version: "old".into(),
            target: "test".into(),
            relative_path: "old/claude".into(),
            sha256: "hash".into(),
            len: 1,
            mtime_unix_nanos: 1,
            source_url: "https://example.invalid".into(),
        };
        write_record_atomic(&path, &record).unwrap();
        record.version = "new".into();
        write_record_atomic(&path, &record).unwrap();
        assert_eq!(read_record(&path).unwrap().version, "new");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
