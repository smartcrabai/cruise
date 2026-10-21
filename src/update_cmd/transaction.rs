use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::TRANSACTION_FILE;
use super::release::ReleaseRuntime;
use crate::error::{CruiseError, Result};
use crate::install_identity::{self, ACTIVE_RUNTIME_FILE, InstallBaseline, ManagedRuntime};

const TRANSACTION_SCHEMA: u32 = 1;
const LOCK_FILE: &str = ".cruise-update.lock";
const STAGING_PREFIX: &str = ".cruise-update-staging-";
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_POINTER_BYTES: u64 = 4096;
const MAX_TRANSACTION_BYTES: u64 = 1024 * 1024;
const MAX_MANAGED_RUNTIME_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Transaction {
    schema_version: u32,
    installation_id: String,
    #[serde(rename = "transaction_id")]
    id: String,
    phase: String,
    baseline: InstallBaseline,
    current_version: String,
    target_version: Option<String>,
    target: Option<String>,
    artifact: Option<String>,
    archive_sha256: Option<String>,
    runtime: Option<String>,
    runtime_sha256: Option<String>,
    previous_active: Option<String>,
    staging: Option<String>,
    error: Option<String>,
}

pub(super) struct Install {
    bin: PathBuf,
    launcher: PathBuf,
    installation_id: String,
    transaction_path: PathBuf,
    baseline: InstallBaseline,
    _lock: File,
}

pub(super) fn verify_current_selection(bin: &Path, launcher: &Path, running: &Path) -> Result<()> {
    read_active(bin)?;
    let selected = install_identity::resolve_runtime(launcher)?;
    let selected = fs::canonicalize(selected)?;
    let running = fs::canonicalize(running)?;
    if selected != running {
        return Err(CruiseError::Other(
            "the runtime executing this command is not selected by the fixed launcher".to_owned(),
        ));
    }
    Ok(())
}
impl Install {
    pub(super) fn acquire(launcher: &Path, bin: &Path, state_dir: &Path) -> Result<Self> {
        let lock_path = bin.join(LOCK_FILE);
        let lock = open_lock(&lock_path)?;
        lock.try_lock_exclusive().map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                CruiseError::Other(format!(
                    "another Cruise update is already running for {}",
                    bin.display()
                ))
            } else {
                error.into()
            }
        })?;

        let baseline = install_identity::install_baseline(bin)?;
        let installation_id = installation_id(launcher)?;
        fs::create_dir_all(state_dir)?;
        ensure_directory(state_dir)?;
        let installations = state_dir.join("installations");
        ensure_directory(&installations)?;
        let install_state = installations.join(&installation_id);
        ensure_directory(&install_state)?;
        let transaction_path = install_state.join(TRANSACTION_FILE);
        ensure_regular_or_absent(&transaction_path)?;

        Ok(Self {
            bin: bin.to_path_buf(),
            launcher: launcher.to_path_buf(),
            installation_id,
            transaction_path,
            baseline,
            _lock: lock,
        })
    }

    pub(super) fn check_current_selection(&self, running: &Path) -> Result<()> {
        verify_current_selection(&self.bin, &self.launcher, running)
    }

    pub(super) fn recover(&self) -> Result<bool> {
        let Some(transaction) = read_transaction(&self.transaction_path)? else {
            return Ok(false);
        };
        self.validate_transaction(&transaction)?;
        let active = read_active(&self.bin)?;
        let previous_selected = active.as_deref() == transaction.previous_active.as_deref();
        let candidate_selected = transaction.runtime.as_deref().is_some_and(|candidate| {
            let candidate_active = format!("{candidate}\n");
            active.as_deref() == Some(candidate_active.as_str())
        });

        if previous_selected {
            if transaction.phase == "completed" {
                return Err(unknown_state(
                    "completed transaction no longer selects its candidate",
                ));
            }
            if matches!(transaction.phase.as_str(), "ready" | "activating") {
                self.validate_candidate(&transaction)?;
            }
            self.ensure_owned_transaction(&transaction)?;
            self.cleanup_staging(transaction.staging.as_deref())?;
            self.remove_transaction(&transaction)?;
            return Ok(false);
        }

        if candidate_selected
            && matches!(
                transaction.phase.as_str(),
                "activating" | "completed" | "failed"
            )
        {
            self.validate_candidate(&transaction)?;
            match probe_launcher(
                &self.launcher,
                transaction.target_version.as_deref().unwrap_or(""),
            ) {
                Ok(()) => self.complete_recovered(transaction),
                Err(probe_error) => {
                    let rollback = self.rollback(&transaction);
                    match rollback {
                        Ok(()) => {
                            let mut failed = transaction;
                            "failed".clone_into(&mut failed.phase);
                            failed.error = Some(format!(
                                "interrupted update failed its launcher probe and was rolled back: {probe_error}"
                            ));
                            self.write_transaction(&failed)?;
                            return Err(CruiseError::Other(failed.error.unwrap_or_default()));
                        }
                        Err(rollback_error) => {
                            return Err(CruiseError::Other(format!(
                                "interrupted update launcher probe failed ({probe_error}); rollback also failed ({rollback_error}); current runtime selection was left unchanged and needs manual recovery"
                            )));
                        }
                    }
                }
            }
            return Ok(true);
        }

        Err(unknown_state(
            "active runtime selection does not match either the recorded prior selection or candidate",
        ))
    }

    pub(super) fn begin(&self, current_version: &str) -> Result<(Transaction, PathBuf)> {
        ensure_regular_or_absent(&self.transaction_path)?;
        if self.transaction_path.exists() {
            return Err(unknown_state("transaction appeared after recovery"));
        }
        let previous_active = read_active(&self.bin)?;
        let transaction_id = Uuid::new_v4().to_string();
        let staging_name = format!("{STAGING_PREFIX}{transaction_id}");
        let mut transaction = Transaction {
            schema_version: TRANSACTION_SCHEMA,
            installation_id: self.installation_id.clone(),
            id: transaction_id,
            phase: "checking".to_owned(),
            baseline: self.baseline.clone(),
            current_version: current_version.to_owned(),
            target_version: None,
            target: None,
            artifact: None,
            archive_sha256: None,
            runtime: None,
            runtime_sha256: None,
            previous_active,
            staging: Some(staging_name.clone()),
            error: None,
        };
        self.write_transaction(&transaction)?;
        let staging = self.bin.join(&staging_name);
        fs::create_dir(&staging)?;
        sync_directory(&self.bin)?;
        "staging".clone_into(&mut transaction.phase);
        self.write_transaction(&transaction)?;
        Ok((transaction, staging))
    }

    pub(super) fn no_update(&self, transaction: &Transaction) -> Result<()> {
        self.ensure_owned_transaction(transaction)?;
        self.cleanup_staging(transaction.staging.as_deref())?;
        self.remove_transaction(transaction)
    }

    pub(super) fn fail(&self, transaction: &mut Transaction, error: &str) -> Result<()> {
        "failed".clone_into(&mut transaction.phase);
        transaction.error = Some(error.to_owned());
        self.write_transaction(transaction)
    }

    pub(super) fn apply(
        &self,
        transaction: &mut Transaction,
        staging: &Path,
        release: ReleaseRuntime,
    ) -> Result<()> {
        let (runtime_path, target_version) = self.stage_candidate(transaction, staging, release)?;
        probe_version(&runtime_path, &target_version)?;
        self.validate_candidate(transaction)?;
        self.ensure_unchanged(transaction)?;
        "ready".clone_into(&mut transaction.phase);
        self.write_transaction(transaction)?;
        checkpoint("ready");

        "activating".clone_into(&mut transaction.phase);
        self.write_transaction(transaction)?;
        checkpoint("activating");
        self.ensure_unchanged(transaction)?;
        self.validate_candidate(transaction)?;
        let runtime_name = transaction
            .runtime
            .as_deref()
            .ok_or_else(|| unknown_state("transaction has no candidate runtime"))?;
        write_active(&self.bin, runtime_name)?;
        checkpoint("committed");

        match probe_after_activation(&self.launcher, &target_version) {
            Ok(()) => {
                "completed".clone_into(&mut transaction.phase);
                if let Err(error) = self.write_transaction(transaction) {
                    eprintln!(
                        "Update applied, but its transaction record could not be marked complete: {error}"
                    );
                    return Ok(());
                }
                checkpoint("completed");
                if let Err(error) = self
                    .ensure_owned_transaction(transaction)
                    .and_then(|()| self.cleanup_staging(transaction.staging.as_deref()))
                    .and_then(|()| self.remove_transaction(transaction))
                {
                    eprintln!("Update applied, but transaction cleanup is incomplete: {error}");
                }
                println!("Updated Cruise to {target_version}.");
                Ok(())
            }
            Err(probe_error) => {
                let rollback = self.rollback(transaction);
                match rollback {
                    Ok(()) => {
                        "failed".clone_into(&mut transaction.phase);
                        transaction.error = Some(format!(
                            "the fixed launcher failed to start version {target_version} after activation: {probe_error}; the previous runtime was restored"
                        ));
                        self.write_transaction(transaction)?;
                        Err(CruiseError::Other(
                            transaction.error.clone().unwrap_or_default(),
                        ))
                    }
                    Err(rollback_error) => {
                        transaction.error = Some(format!(
                            "the fixed launcher failed to start version {target_version} after activation ({probe_error}); rollback failed ({rollback_error}); current selection needs manual recovery"
                        ));
                        let _ = self.write_transaction(transaction);
                        Err(CruiseError::Other(
                            transaction.error.clone().unwrap_or_default(),
                        ))
                    }
                }
            }
        }
    }

    fn stage_candidate(
        &self,
        transaction: &mut Transaction,
        staging: &Path,
        release: ReleaseRuntime,
    ) -> Result<(PathBuf, String)> {
        let staging_metadata = fs::symlink_metadata(staging)?;
        if !staging_metadata.file_type().is_dir() {
            return Err(unknown_state(
                "release staging path is not a real directory",
            ));
        }
        let staging = fs::canonicalize(staging)?;
        let recorded_staging = self.bin.join(
            transaction
                .staging
                .as_deref()
                .ok_or_else(|| unknown_state("transaction has no staging directory"))?,
        );
        if fs::canonicalize(recorded_staging)? != staging {
            return Err(unknown_state("release staging path changed"));
        }
        ensure_regular(&release.runtime)?;
        let runtime_source = fs::canonicalize(release.runtime)?;
        if runtime_source.parent() != Some(staging.as_path())
            || runtime_source.file_name() != Some(OsStr::new(install_identity::runtime_name()))
        {
            return Err(unknown_state(
                "release runtime is outside the owned staging directory",
            ));
        }

        "verifying".clone_into(&mut transaction.phase);
        let target_version = release.version;
        transaction.target_version = Some(target_version.clone());
        transaction.target = Some(release.target.clone());
        transaction.artifact = Some(release.artifact.clone());
        transaction.archive_sha256 = Some(release.archive_sha256.clone());
        self.write_transaction(transaction)?;

        let runtime_name = format!(
            "{}{}-{}{}",
            install_identity::MANAGED_RUNTIME_PREFIX,
            target_version,
            Uuid::new_v4().simple(),
            if cfg!(windows) { ".exe" } else { "" }
        );
        if !install_identity::valid_runtime_name(&runtime_name) {
            return Err(CruiseError::Other(
                "generated runtime filename is invalid".to_owned(),
            ));
        }
        let runtime_path = self.bin.join(&runtime_name);
        copy_candidate(&runtime_source, &runtime_path, &release.runtime_sha256)?;
        transaction.runtime = Some(runtime_name.clone());
        transaction.runtime_sha256 = Some(release.runtime_sha256.clone());
        write_sidecar(
            &runtime_path,
            &ManagedRuntime {
                schema_version: 1,
                runtime: runtime_name,
                version: target_version.clone(),
                target: release.target,
                sha256: release.runtime_sha256,
                baseline: self.baseline.clone(),
            },
        )?;
        sync_directory(&self.bin)?;
        Ok((runtime_path, target_version))
    }

    fn complete_recovered(&self, mut transaction: Transaction) {
        "completed".clone_into(&mut transaction.phase);
        transaction.error = None;
        if let Err(error) = self.write_transaction(&transaction) {
            eprintln!(
                "Update is active and launcher-verified, but its transaction record could not be completed: {error}"
            );
            return;
        }
        if let Err(error) = self
            .ensure_owned_transaction(&transaction)
            .and_then(|()| self.cleanup_staging(transaction.staging.as_deref()))
            .and_then(|()| self.remove_transaction(&transaction))
        {
            eprintln!(
                "Update is active and launcher-verified, but transaction cleanup is incomplete: {error}"
            );
        }
    }

    fn validate_transaction(&self, transaction: &Transaction) -> Result<()> {
        let invalid = || unknown_state("transaction fields do not match a supported update state");
        if transaction.schema_version != TRANSACTION_SCHEMA
            || transaction.installation_id != self.installation_id
            || transaction.baseline != self.baseline
            || Uuid::parse_str(&transaction.id)
                .ok()
                .is_none_or(|id| id.to_string() != transaction.id)
        {
            return Err(invalid());
        }
        if semver::Version::parse(&transaction.current_version)
            .ok()
            .is_none_or(|version| !version.pre.is_empty())
        {
            return Err(invalid());
        }
        let expected_staging = format!("{STAGING_PREFIX}{}", transaction.id);
        if transaction.staging.as_deref() != Some(expected_staging.as_str()) {
            return Err(invalid());
        }
        if transaction
            .previous_active
            .as_deref()
            .is_some_and(|name| !valid_active_name(name.trim()))
        {
            return Err(invalid());
        }

        let release_fields = match (
            transaction.target_version.as_deref(),
            transaction.target.as_deref(),
            transaction.artifact.as_deref(),
            transaction.archive_sha256.as_deref(),
        ) {
            (None, None, None, None) => false,
            (Some(version), Some(target), Some(artifact), Some(digest))
                if semver::Version::parse(version).is_ok_and(|version| version.pre.is_empty())
                    && target == super::release::BUILD_TARGET
                    && safe_basename(artifact)
                    && valid_sha256(digest) =>
            {
                true
            }
            _ => return Err(invalid()),
        };
        let candidate_fields = match (
            transaction.runtime.as_deref(),
            transaction.runtime_sha256.as_deref(),
        ) {
            (None, None) => false,
            (Some(runtime), Some(digest))
                if release_fields
                    && valid_candidate_name(
                        runtime,
                        transaction.target_version.as_deref().unwrap_or_default(),
                    )
                    && valid_sha256(digest) =>
            {
                true
            }
            _ => return Err(invalid()),
        };

        match transaction.phase.as_str() {
            "checking" | "staging"
                if !release_fields && !candidate_fields && transaction.error.is_none() => {}
            "verifying" if release_fields && !candidate_fields && transaction.error.is_none() => {}
            "ready" | "activating" | "completed"
                if release_fields && candidate_fields && transaction.error.is_none() => {}
            "failed"
                if transaction
                    .error
                    .as_deref()
                    .is_some_and(|error| !error.is_empty()) => {}
            _ => return Err(invalid()),
        }
        Ok(())
    }
    fn validate_transition(&self, current: &Transaction, next: &Transaction) -> Result<()> {
        self.validate_transaction(current)?;
        self.validate_transaction(next)?;
        if current.schema_version != next.schema_version
            || current.installation_id != next.installation_id
            || current.id != next.id
            || current.baseline != next.baseline
            || current.current_version != next.current_version
            || current.previous_active != next.previous_active
            || current.staging != next.staging
            || current
                .target_version
                .as_ref()
                .is_some_and(|value| next.target_version.as_ref() != Some(value))
            || current
                .target
                .as_ref()
                .is_some_and(|value| next.target.as_ref() != Some(value))
            || current
                .artifact
                .as_ref()
                .is_some_and(|value| next.artifact.as_ref() != Some(value))
            || current
                .archive_sha256
                .as_ref()
                .is_some_and(|value| next.archive_sha256.as_ref() != Some(value))
            || current
                .runtime
                .as_ref()
                .is_some_and(|value| next.runtime.as_ref() != Some(value))
            || current
                .runtime_sha256
                .as_ref()
                .is_some_and(|value| next.runtime_sha256.as_ref() != Some(value))
        {
            return Err(unknown_state(
                "transaction contents changed outside this update",
            ));
        }
        let valid_phase_change = matches!(
            (current.phase.as_str(), next.phase.as_str()),
            ("checking", "staging" | "failed")
                | ("staging", "verifying" | "failed")
                | ("verifying", "ready" | "failed")
                | ("ready", "activating" | "failed")
                | (
                    "activating" | "completed" | "failed",
                    "completed" | "failed"
                )
        );
        if !valid_phase_change
            || (current.phase == "failed" && next.phase == "failed" && current.error != next.error)
        {
            return Err(unknown_state(
                "transaction phase changed outside the update protocol",
            ));
        }
        Ok(())
    }

    fn validate_candidate(&self, transaction: &Transaction) -> Result<()> {
        let runtime_name = transaction
            .runtime
            .as_deref()
            .ok_or_else(|| unknown_state("selected candidate is missing from the transaction"))?;
        let runtime_path = self.bin.join(runtime_name);
        ensure_regular(&runtime_path)?;
        let sidecar_path = install_identity::metadata_path(&runtime_path);
        ensure_regular(&sidecar_path)?;
        let sidecar: ManagedRuntime =
            serde_json::from_slice(&read_file_limited(&sidecar_path, 64 * 1024)?)?;
        if sidecar.schema_version != 1
            || sidecar.runtime != runtime_name
            || Some(&sidecar.version) != transaction.target_version.as_ref()
            || Some(&sidecar.target) != transaction.target.as_ref()
            || Some(&sidecar.sha256) != transaction.runtime_sha256.as_ref()
            || sidecar.baseline != self.baseline
        {
            return Err(unknown_state(
                "candidate sidecar does not match the transaction",
            ));
        }
        let expected = transaction
            .runtime_sha256
            .as_deref()
            .ok_or_else(|| unknown_state("candidate digest is missing from the transaction"))?;
        if hash_file(&runtime_path)? != expected {
            return Err(unknown_state("selected candidate digest no longer matches"));
        }
        Ok(())
    }

    fn ensure_unchanged(&self, transaction: &Transaction) -> Result<()> {
        if install_identity::install_baseline(&self.bin)? != self.baseline
            || read_active(&self.bin)? != transaction.previous_active
        {
            return Err(CruiseError::Other(
                "the fixed launcher/runtime pair or active selection changed during update; no update was activated".to_owned(),
            ));
        }
        Ok(())
    }

    fn rollback(&self, transaction: &Transaction) -> Result<()> {
        if install_identity::install_baseline(&self.bin)? != self.baseline {
            return Err(unknown_state(
                "fixed install baseline changed before rollback",
            ));
        }
        let candidate = transaction
            .runtime
            .as_deref()
            .ok_or_else(|| unknown_state("cannot roll back without a recorded candidate"))?;
        let current = read_active(&self.bin)?;
        let candidate_active = format!("{candidate}\n");
        if current.as_deref() != Some(candidate_active.as_str()) {
            return Err(unknown_state("active selection changed before rollback"));
        }
        if let Some(previous) = transaction.previous_active.as_deref() {
            write_pointer_bytes(&self.bin, previous.as_bytes())?;
        } else {
            let fixed_runtime = self.bin.join(install_identity::runtime_name());
            ensure_regular(&fixed_runtime)?;
            fs::remove_file(self.bin.join(ACTIVE_RUNTIME_FILE))?;
            sync_directory(&self.bin)?;
        }
        probe_launcher(&self.launcher, &transaction.current_version).map_err(|error| {
            CruiseError::Other(format!(
                "previous selection was restored, but the fixed launcher did not verify version {}: {error}",
                transaction.current_version
            ))
        })
    }

    fn write_transaction(&self, transaction: &Transaction) -> Result<()> {
        ensure_regular_or_absent(&self.transaction_path)?;
        match read_transaction(&self.transaction_path)? {
            Some(current) => self.validate_transition(&current, transaction)?,
            None if transaction.phase == "checking" => self.validate_transaction(transaction)?,
            None => return Err(unknown_state("transaction disappeared during update")),
        }
        let bytes = serde_json::to_vec_pretty(transaction)?;
        atomic_replace(&self.transaction_path, &bytes)
    }

    fn ensure_owned_transaction(&self, transaction: &Transaction) -> Result<()> {
        match read_transaction(&self.transaction_path)? {
            Some(current) if &current == transaction => Ok(()),
            _ => Err(unknown_state("transaction changed before cleanup")),
        }
    }

    fn remove_transaction(&self, transaction: &Transaction) -> Result<()> {
        ensure_regular_or_absent(&self.transaction_path)?;
        match read_transaction(&self.transaction_path)? {
            None => return Ok(()),
            Some(current) if &current == transaction => {}
            Some(_) => {
                return Err(unknown_state(
                    "refusing to remove a transaction that changed externally",
                ));
            }
        }
        fs::remove_file(&self.transaction_path)?;
        sync_directory(
            self.transaction_path
                .parent()
                .unwrap_or_else(|| Path::new(".")),
        )
    }

    fn cleanup_staging(&self, name: Option<&str>) -> Result<()> {
        let Some(name) = name else { return Ok(()) };
        if !name.starts_with(STAGING_PREFIX) || Path::new(name).components().count() != 1 {
            return Err(unknown_state("refusing to clean an unowned staging path"));
        }
        let path = self.bin.join(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
            Ok(metadata) if metadata.file_type().is_dir() => {
                for entry in fs::read_dir(&path)? {
                    let entry = entry?;
                    let name = entry.file_name();
                    if (name != OsStr::new(".cruise-update-archive")
                        && name != OsStr::new(install_identity::runtime_name()))
                        || !entry.file_type()?.is_file()
                    {
                        return Err(unknown_state("staging directory contains an unowned path"));
                    }
                }
                fs::remove_dir_all(path)?;
                sync_directory(&self.bin)
            }
            Ok(_) => Err(unknown_state("staging path is not a real directory")),
        }
    }
}

fn installation_id(launcher: &Path) -> Result<String> {
    let canonical = fs::canonicalize(launcher)?;
    let mut hash = Sha256::new();
    hash.update(b"cruise-installation-v1\0");
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hash.update(canonical.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        for unit in canonical.as_os_str().encode_wide() {
            hash.update(unit.to_le_bytes());
        }
    }
    Ok(digest_hex(hash.finalize()))
}

fn open_lock(path: &Path) -> Result<File> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Err(unknown_state("update lock path is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(unknown_state("opened update lock is not a regular file"));
    }
    Ok(file)
}

fn ensure_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(unknown_state(&format!(
            "{} is not a real directory",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn ensure_regular(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(unknown_state(&format!(
            "{} is not a regular file",
            path.display()
        ))),
        Err(error) => Err(error.into()),
    }
}
fn open_regular(path: &Path) -> Result<File> {
    ensure_regular(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(unknown_state("opened path is not a regular file"));
    }
    Ok(file)
}

fn read_file_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = open_regular(path)?;
    if file.metadata()?.len() > limit {
        return Err(unknown_state(&format!(
            "{} exceeds its size limit",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(unknown_state(&format!(
            "{} exceeds its size limit",
            path.display()
        )));
    }
    Ok(bytes)
}

fn ensure_regular_or_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(unknown_state(&format!(
            "{} is not a regular file",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn read_transaction(path: &Path) -> Result<Option<Transaction>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(unknown_state("transaction path is not a regular file"));
        }
        Ok(_) => {}
    }
    let bytes = read_file_limited(path, MAX_TRANSACTION_BYTES)?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn read_active(bin: &Path) -> Result<Option<String>> {
    let path = bin.join(ACTIVE_RUNTIME_FILE);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_file() => Err(unknown_state(
            "active runtime pointer is not a regular file",
        )),
        Ok(metadata) if metadata.len() > MAX_POINTER_BYTES => Err(unknown_state(
            "active runtime pointer exceeds its size limit",
        )),
        Ok(_) => {
            let bytes = read_file_limited(&path, MAX_POINTER_BYTES)?;
            let content = String::from_utf8(bytes)
                .map_err(|_| unknown_state("active runtime pointer is not UTF-8"))?;
            let name = content.trim();
            if !valid_active_name(name) {
                return Err(unknown_state(
                    "active runtime pointer contains an invalid basename",
                ));
            }
            let runtime = bin.join(name);
            ensure_regular(&runtime)?;
            Ok(Some(content))
        }
    }
}

fn valid_active_name(name: &str) -> bool {
    install_identity::valid_runtime_name(name)
}
fn safe_basename(name: &str) -> bool {
    !name.is_empty()
        && !name
            .bytes()
            .any(|byte| byte == b'/' || byte == b'\\' || byte.is_ascii_control())
        && Path::new(name).components().count() == 1
        && Path::new(name).file_name() == Some(OsStr::new(name))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_candidate_name(name: &str, version: &str) -> bool {
    if !install_identity::valid_runtime_name(name) {
        return false;
    }
    let prefix = format!("{}{version}-", install_identity::MANAGED_RUNTIME_PREFIX);
    let Some(suffix) = name.strip_prefix(&prefix) else {
        return false;
    };
    let suffix = if cfg!(windows) {
        let Some(suffix) = suffix.strip_suffix(".exe") else {
            return false;
        };
        suffix
    } else {
        suffix
    };
    Uuid::parse_str(suffix).is_ok_and(|id| id.simple().to_string() == suffix)
}

fn write_active(bin: &Path, name: &str) -> Result<()> {
    if !install_identity::valid_runtime_name(name) {
        return Err(unknown_state(
            "refusing to activate an invalid runtime basename",
        ));
    }
    let runtime = bin.join(name);
    ensure_regular(&runtime)?;
    write_pointer_bytes(bin, format!("{name}\n").as_bytes())
}

fn write_pointer_bytes(bin: &Path, bytes: &[u8]) -> Result<()> {
    let path = bin.join(ACTIVE_RUNTIME_FILE);
    ensure_regular_or_absent(&path)?;
    atomic_replace(&path, bytes)
}

fn write_sidecar(runtime: &Path, metadata: &ManagedRuntime) -> Result<()> {
    let path = install_identity::metadata_path(runtime);
    ensure_regular_or_absent(&path)?;
    if path.exists() {
        return Err(unknown_state(
            "refusing to overwrite an existing runtime sidecar",
        ));
    }
    let bytes = serde_json::to_vec_pretty(metadata)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    sync_directory(path.parent().unwrap_or_else(|| Path::new(".")))
}

fn copy_candidate(source: &Path, destination: &Path, expected_hash: &str) -> Result<()> {
    ensure_regular(source)?;
    if destination.exists() || fs::symlink_metadata(destination).is_ok() {
        return Err(unknown_state("refusing to overwrite a managed runtime"));
    }
    let mut input = open_regular(source)?;
    if input.metadata()?.len() > MAX_MANAGED_RUNTIME_BYTES {
        return Err(unknown_state("runtime candidate exceeds its size limit"));
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o755);
    }
    let mut output = options.open(destination)?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .filter(|size| *size <= MAX_MANAGED_RUNTIME_BYTES)
            .ok_or_else(|| unknown_state("runtime candidate exceeds its size limit"))?;
        digest.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    output.sync_all()?;
    let actual = digest_hex(digest.finalize());
    if actual != expected_hash {
        return Err(CruiseError::Other(format!(
            "runtime digest changed while installing (expected {expected_hash}, got {actual})"
        )));
    }
    sync_directory(destination.parent().unwrap_or_else(|| Path::new(".")))
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = open_regular(path)?;
    if file.metadata()?.len() > MAX_MANAGED_RUNTIME_BYTES {
        return Err(unknown_state("runtime candidate exceeds its size limit"));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .filter(|size| *size <= MAX_MANAGED_RUNTIME_BYTES)
            .ok_or_else(|| unknown_state("runtime candidate exceeds its size limit"))?;
        digest.update(&buffer[..count]);
    }
    Ok(digest_hex(digest.finalize()))
}
fn digest_hex(digest: impl IntoIterator<Item = u8>) -> String {
    let mut output = String::with_capacity(64);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure_regular_or_absent(path)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp = parent.join(format!(".cruise-update-{}.tmp", Uuid::new_v4().simple()));
    let mut created = false;
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        created = true;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace_file(&temp, path)?;
        sync_directory(parent)
    })();
    if result.is_err() && created {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
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
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn probe_version(runtime: &Path, expected: &str) -> Result<()> {
    probe(runtime, expected)
}

fn probe_launcher(launcher: &Path, expected: &str) -> Result<()> {
    probe(launcher, expected)
}

fn probe_after_activation(launcher: &Path, expected: &str) -> Result<()> {
    #[cfg(all(feature = "test-utils", debug_assertions))]
    if std::env::var_os("CRUISE_UPDATE_TEST_FAIL_POSTPROBE").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        return Err(CruiseError::Other(
            "injected post-activation launcher probe failure".to_owned(),
        ));
    }
    probe(launcher, expected)
}

fn probe(executable: &Path, expected: &str) -> Result<()> {
    let mut child = Command::new(executable)
        .arg("--version")
        .env_remove("CRUISE_RUNTIME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            if !output.status.success() {
                return Err(CruiseError::Other(format!(
                    "version probe exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            let actual = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let expected_line = format!("cruise {expected}");
            if actual != expected_line {
                return Err(CruiseError::Other(format!(
                    "version probe returned {actual:?}, expected {expected_line:?}"
                )));
            }
            return Ok(());
        }
        if started.elapsed() >= PROBE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CruiseError::Other(format!(
                "version probe timed out after {} seconds",
                PROBE_TIMEOUT.as_secs()
            )));
        }
        thread::sleep(Duration::from_millis(40));
    }
}

fn unknown_state(message: &str) -> CruiseError {
    CruiseError::Other(format!("unsafe update transaction state: {message}"))
}

fn checkpoint(name: &str) {
    #[cfg(all(feature = "test-utils", debug_assertions))]
    if std::env::var("CRUISE_UPDATE_TEST_STOP_AFTER")
        .ok()
        .as_deref()
        == Some(name)
    {
        std::process::exit(86);
    }
    #[cfg(not(all(feature = "test-utils", debug_assertions)))]
    let _ = name;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn test_install(root: &Path) -> (PathBuf, PathBuf) {
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap_or_else(|error| panic!("{error}"));
        let launcher = bin.join(install_identity::launcher_name());
        let runtime = bin.join(install_identity::runtime_name());
        fs::write(&launcher, b"launcher").unwrap_or_else(|error| panic!("{error}"));
        fs::write(&runtime, b"runtime").unwrap_or_else(|error| panic!("{error}"));
        (bin, launcher)
    }

    #[test]
    fn lock_is_exclusive_for_the_same_install_bin() {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let lock_path = temp.path().join(LOCK_FILE);
        let first = open_lock(&lock_path).unwrap_or_else(|error| panic!("{error}"));
        first
            .try_lock_exclusive()
            .unwrap_or_else(|error| panic!("{error}"));
        let second = open_lock(&lock_path).unwrap_or_else(|error| panic!("{error}"));
        assert!(second.try_lock_exclusive().is_err());
    }

    #[test]
    fn lock_and_active_pointer_symlinks_are_rejected() {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let target = temp.path().join("target");
        fs::write(&target, b"x").unwrap_or_else(|error| panic!("{error}"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, temp.path().join(LOCK_FILE))
                .unwrap_or_else(|error| panic!("{error}"));
            assert!(open_lock(&temp.path().join(LOCK_FILE)).is_err());
            let transaction_path = temp.path().join(TRANSACTION_FILE);
            std::os::unix::fs::symlink(&target, &transaction_path)
                .unwrap_or_else(|error| panic!("{error}"));
            assert!(ensure_regular_or_absent(&transaction_path).is_err());
            let bin = temp.path().join("bin");
            fs::create_dir(&bin).unwrap_or_else(|error| panic!("{error}"));
            std::os::unix::fs::symlink(&target, bin.join(ACTIVE_RUNTIME_FILE))
                .unwrap_or_else(|error| panic!("{error}"));
            assert!(read_active(&bin).is_err());
        }
    }

    #[test]
    fn corrupt_or_unknown_transaction_is_not_modified() {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let (bin, launcher) = test_install(temp.path());
        let state_dir = temp.path().join("state");
        fs::create_dir(&state_dir).unwrap_or_else(|error| panic!("{error}"));
        let install =
            Install::acquire(&launcher, &bin, &state_dir).unwrap_or_else(|error| panic!("{error}"));
        let contents = br#"{"schema_version":99}"#;
        fs::write(&install.transaction_path, contents).unwrap_or_else(|error| panic!("{error}"));
        assert!(install.recover().is_err());
        assert_eq!(
            fs::read(&install.transaction_path).unwrap_or_default(),
            contents
        );
    }
    #[test]
    fn externally_changed_valid_transaction_is_not_replaced() {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let (bin, launcher) = test_install(temp.path());
        let state_dir = temp.path().join("state");
        fs::create_dir(&state_dir).unwrap_or_else(|error| panic!("{error}"));
        let install =
            Install::acquire(&launcher, &bin, &state_dir).unwrap_or_else(|error| panic!("{error}"));
        let (mut transaction, _staging) = install
            .begin("0.2.16")
            .unwrap_or_else(|error| panic!("{error}"));
        let mut changed: serde_json::Value =
            serde_json::from_slice(&fs::read(&install.transaction_path).unwrap_or_default())
                .unwrap_or_else(|error| panic!("{error}"));
        changed["current_version"] = serde_json::Value::String("0.2.17".to_owned());
        let changed = serde_json::to_vec(&changed).unwrap_or_default();
        fs::write(&install.transaction_path, &changed).unwrap_or_else(|error| panic!("{error}"));
        assert!(install.fail(&mut transaction, "failure").is_err());
        assert_eq!(
            fs::read(&install.transaction_path).unwrap_or_default(),
            changed
        );
    }

    #[test]
    fn pointer_commit_and_rollback_replace_atomically_without_delete_first() {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).unwrap_or_else(|error| panic!("{error}"));
        let candidate = format!("{}candidate", install_identity::MANAGED_RUNTIME_PREFIX);
        let previous = format!("{}previous", install_identity::MANAGED_RUNTIME_PREFIX);
        fs::write(bin.join(&candidate), b"candidate").unwrap_or_else(|error| panic!("{error}"));
        fs::write(bin.join(&previous), b"previous").unwrap_or_else(|error| panic!("{error}"));
        write_active(&bin, &previous).unwrap_or_else(|error| panic!("{error}"));
        write_active(&bin, &candidate).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            read_active(&bin).ok().flatten(),
            Some(format!("{candidate}\n"))
        );
        assert!(bin.join(&previous).is_file());
        write_pointer_bytes(&bin, format!("{previous}\n").as_bytes())
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            read_active(&bin).ok().flatten(),
            Some(format!("{previous}\n"))
        );
        assert!(bin.join(&candidate).is_file());
    }
    #[cfg(unix)]
    #[test]
    fn staging_symlink_is_not_removed() {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let (bin, launcher) = test_install(temp.path());
        let state_dir = temp.path().join("state");
        fs::create_dir(&state_dir).unwrap_or_else(|error| panic!("{error}"));
        let install =
            Install::acquire(&launcher, &bin, &state_dir).unwrap_or_else(|error| panic!("{error}"));
        let target = temp.path().join("outside");
        fs::write(&target, b"leave untouched").unwrap_or_else(|error| panic!("{error}"));
        let name = format!("{STAGING_PREFIX}test");
        std::os::unix::fs::symlink(&target, bin.join(&name))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(install.cleanup_staging(Some(&name)).is_err());
        assert_eq!(fs::read(target).unwrap_or_default(), b"leave untouched");
    }

    #[test]
    fn installation_lock_and_transaction_state_are_scoped_under_the_bin_and_state_id() {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let (bin, launcher) = test_install(temp.path());
        let state_dir = temp.path().join("state");
        fs::create_dir(&state_dir).unwrap_or_else(|error| panic!("{error}"));
        let install =
            Install::acquire(&launcher, &bin, &state_dir).unwrap_or_else(|error| panic!("{error}"));
        assert!(bin.join(LOCK_FILE).is_file());
        assert!(
            install
                .transaction_path
                .starts_with(state_dir.join("installations"))
        );
        assert_eq!(
            install
                .transaction_path
                .file_name()
                .and_then(|name| name.to_str()),
            Some(TRANSACTION_FILE)
        );
    }
}
