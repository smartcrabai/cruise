//! Keeps the user's jcode OAuth stores authoritative for private session homes.
//!
//! jcode links `openai-auth.json` and `auth.json` into each private home, but
//! persists a refresh with a temporary file and `rename`, which replaces the
//! link instead of writing through it. Providers rotate refresh tokens, so the
//! only valid refresh token would then be stranded in a private home that is
//! deleted after the attempt, and the next launch fails with a reused token.
//!
//! Cruise therefore refreshes ahead of expiry in the user's home, serialized
//! across Cruise processes, so private runtimes (which refresh only in the last
//! five minutes) never rotate a shared login themselves; and reclaims any
//! rotation a private runtime performed anyway.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{
    JCODE_CHECK_UPDATES_ENV, JCODE_HOME_ENV, JcodeRunnerConfig, NO_TELEMETRY_ENV, SESSION_HOME_DIR,
    ensure_private_dir, set_private_file,
};

/// Refresh this long before expiry: longer than an attempt is expected to run,
/// shorter than Claude's eight-hour access token.
const REFRESH_AHEAD_MS: i64 = 2 * 60 * 60 * 1000;
/// jcode refreshes inside the last five minutes; below this, a launched private
/// runtime would rotate the login itself.
const MIN_LAUNCH_VALIDITY_MS: i64 = 10 * 60 * 1000;
const AUTH_TEST_TIMEOUT: Duration = Duration::from_secs(120);
const REFRESH_LOCK_FILE: &str = ".cruise-credential-refresh.lock";
const DETAIL_LIMIT: usize = 400;

/// A jcode-owned OAuth account store shared with private session homes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OAuthStore {
    OpenAi,
    Anthropic,
}

impl OAuthStore {
    /// The store an explicit `provider/` model prefix authenticates with.
    pub(crate) fn for_provider(provider: &str) -> Option<Self> {
        match provider {
            "claude" | "claude-oauth" | "claude-subprocess" => Some(Self::Anthropic),
            "openai" | "openai-oauth" => Some(Self::OpenAi),
            _ => None,
        }
    }

    pub(crate) const fn file_name(self) -> &'static str {
        match self {
            Self::OpenAi => "openai-auth.json",
            Self::Anthropic => "auth.json",
        }
    }

    /// The jcode provider name for `login` and `auth-test`.
    pub(crate) const fn login_provider(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "claude",
        }
    }

    const fn accounts_key(self) -> &'static str {
        match self {
            Self::OpenAi => "openai_accounts",
            Self::Anthropic => "anthropic_accounts",
        }
    }

    const fn active_key(self) -> &'static str {
        match self {
            Self::OpenAi => "active_openai_account",
            Self::Anthropic => "active_anthropic_account",
        }
    }

    const fn expiry_key(self) -> &'static str {
        match self {
            Self::OpenAi => "expires_at",
            Self::Anthropic => "expires",
        }
    }

    fn account_expiry_ms(self, account: &Value) -> Option<i64> {
        account
            .get(self.expiry_key())
            .and_then(Value::as_i64)
            .map(crate::agent_auth::normalize_ms)
    }

    /// Access-token expiry of the account jcode uses: the active label, else the
    /// first account. `None` when the store is absent or unreadable.
    fn active_expiry_ms(self, path: &Path) -> Option<i64> {
        let store = read_json(path)?;
        let accounts = store.get(self.accounts_key())?.as_array()?;
        let active = store.get(self.active_key()).and_then(Value::as_str);
        let account = active
            .and_then(|label| {
                accounts
                    .iter()
                    .find(|account| label_of(account) == Some(label))
            })
            .or_else(|| accounts.first())?;
        self.account_expiry_ms(account)
    }
}

/// Refresh the attempt's OAuth login in the user's jcode home before a private
/// runtime could need to.
///
/// Unknown providers and absent or unreadable stores pass: jcode reports those
/// itself. A failed refresh only fails the attempt when the current access
/// token would not outlive jcode's own refresh window.
pub(super) fn refresh_ahead(
    config: &JcodeRunnerConfig,
    source_home: &Path,
) -> std::result::Result<(), String> {
    let Some(store) = config
        .provider
        .as_deref()
        .and_then(OAuthStore::for_provider)
    else {
        return Ok(());
    };
    let path = source_home.join(store.file_name());
    if !needs_refresh(store, &path) {
        return Ok(());
    }
    let root = source_home.join(SESSION_HOME_DIR);
    ensure_private_dir(&root).map_err(|error| error.to_string())?;
    let _lock = lock_exclusive(&root.join(REFRESH_LOCK_FILE))
        .map_err(|error| format!("could not lock jcode credential refresh: {error}"))?;
    // Another attempt may have refreshed while this one waited for the lock.
    if !needs_refresh(store, &path) {
        return Ok(());
    }
    let provider = store.login_provider();
    let detail = match run_auth_test(config, source_home, store) {
        Ok(None) => return Ok(()),
        Ok(Some(detail)) => detail,
        // jcode never reported a refresh outcome; let the runtime try instead.
        Err(error) => {
            eprintln!(
                "cruise: warning: could not refresh jcode provider `{provider}` ahead of expiry: {error}"
            );
            return Ok(());
        }
    };
    match remaining_ms(store, &path) {
        Some(remaining) if remaining < MIN_LAUNCH_VALIDITY_MS => Err(format!(
            "jcode provider `{provider}` authentication expired and could not be refreshed: {detail}; run `jcode login {provider}`"
        )),
        _ => {
            eprintln!(
                "cruise: warning: could not refresh jcode provider `{provider}` ahead of expiry: {detail}"
            );
            Ok(())
        }
    }
}

fn needs_refresh(store: OAuthStore, path: &Path) -> bool {
    remaining_ms(store, path).is_some_and(|remaining| remaining < REFRESH_AHEAD_MS)
}

fn remaining_ms(store: OAuthStore, path: &Path) -> Option<i64> {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis(),
    )
    .ok()?;
    Some(store.active_expiry_ms(path)?.saturating_sub(now))
}

/// Force a refresh through jcode itself against the user's home, so jcode
/// persists it to the real store under its own lock. `Ok(Some(detail))` is a
/// refresh jcode reported as failed; `Err` means no refresh outcome was seen.
fn run_auth_test(
    config: &JcodeRunnerConfig,
    source_home: &Path,
    store: OAuthStore,
) -> std::result::Result<Option<String>, String> {
    let binary = config
        .binary
        .clone()
        .unwrap_or_else(|| PathBuf::from("jcode"));
    let mut command = Command::new(&binary);
    command
        .args([
            "auth-test",
            "--provider",
            store.login_provider(),
            "--no-smoke",
            "--no-tool-smoke",
        ])
        .current_dir(config.cwd.as_deref().unwrap_or(source_home))
        .envs(
            config
                .env
                .iter()
                .filter(|(key, _)| key.as_str() != JCODE_HOME_ENV),
        )
        .env(JCODE_HOME_ENV, source_home)
        .env(NO_TELEMETRY_ENV, "1")
        .env(JCODE_CHECK_UPDATES_ENV, "0")
        .env("JCODE_NO_AUTO_UPDATE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run `{} auth-test`: {error}", binary.display()))?;
    let readers = [
        child
            .stdout
            .take()
            .map(|pipe| std::thread::spawn(move || read_all(pipe))),
        child
            .stderr
            .take()
            .map(|pipe| std::thread::spawn(move || read_all(pipe))),
    ];
    let deadline = Instant::now() + AUTH_TEST_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "jcode auth-test did not finish within {}s",
                    AUTH_TEST_TIMEOUT.as_secs()
                ));
            }
            Err(error) => return Err(format!("could not wait for jcode auth-test: {error}")),
        }
    };
    let output: String = readers
        .into_iter()
        .flatten()
        .filter_map(|reader| reader.join().ok())
        .collect();
    if status.success() {
        Ok(None)
    } else {
        refresh_failure_detail(&output)
            .map(Some)
            .ok_or_else(|| format!("jcode auth-test exited with {status}"))
    }
}

fn read_all(mut pipe: impl std::io::Read) -> String {
    let mut bytes = Vec::new();
    let _ = pipe.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The failed `refresh_probe` step of jcode's auth-test report, on one line.
fn refresh_failure_detail(output: &str) -> Option<String> {
    let start = output.find("✗ refresh_probe")? + "✗ refresh_probe".len();
    let rest = output[start..].trim_start_matches([' ', '—', '-']);
    let end = ["\n✓ ", "\n✗ ", "\nresult:"]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min()
        .unwrap_or(rest.len());
    let mut detail = rest[..end].split_whitespace().collect::<Vec<_>>().join(" ");
    if detail.len() > DETAIL_LIMIT {
        let mut cut = DETAIL_LIMIT;
        while !detail.is_char_boundary(cut) {
            cut -= 1;
        }
        detail.truncate(cut);
        detail.push('…');
    }
    (!detail.is_empty()).then_some(detail)
}

/// Copy refreshed accounts a private runtime stranded in its home back to the
/// user's stores, under jcode's own store lock.
///
/// A store that is still a link was never rewritten. Only accounts already in
/// the user's store are updated, and only with a later access-token expiry, so
/// a concurrent login or logout always wins.
pub(super) fn reclaim_rotated_credentials(
    source_home: &Path,
    storage_home: &Path,
) -> std::io::Result<()> {
    for store in [OAuthStore::OpenAi, OAuthStore::Anthropic] {
        let private = storage_home.join(store.file_name());
        if fs::symlink_metadata(&private).is_ok_and(|metadata| metadata.is_file()) {
            reclaim_store(source_home, store, &private)?;
        }
    }
    Ok(())
}

fn reclaim_store(source_home: &Path, store: OAuthStore, private: &Path) -> std::io::Result<()> {
    let Some(rotated) = read_json(private) else {
        return Ok(());
    };
    let shared = match fs::canonicalize(source_home.join(store.file_name())) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    // The lock path jcode uses for this store, so jcode processes serialize too.
    let lock = fs::canonicalize(source_home)?.join(format!(".{}.lock", store.file_name()));
    let _lock = lock_exclusive(&lock)?;
    let Some(mut current) = read_json(&shared) else {
        return Ok(());
    };
    if !merge_rotated_accounts(store, &mut current, &rotated) {
        return Ok(());
    }
    let contents = serde_json::to_vec(&current).map_err(std::io::Error::other)?;
    write_private_atomic(&shared, &contents)
}

fn merge_rotated_accounts(store: OAuthStore, shared: &mut Value, rotated: &Value) -> bool {
    let Some(rotated_accounts) = rotated.get(store.accounts_key()).and_then(Value::as_array) else {
        return false;
    };
    let Some(shared_accounts) = shared
        .get_mut(store.accounts_key())
        .and_then(Value::as_array_mut)
    else {
        return false;
    };
    let mut changed = false;
    for candidate in rotated_accounts {
        let Some(label) = label_of(candidate) else {
            continue;
        };
        let Some(slot) = shared_accounts
            .iter_mut()
            .find(|account| label_of(account) == Some(label))
        else {
            continue;
        };
        if store.account_expiry_ms(candidate) > store.account_expiry_ms(slot) {
            slot.clone_from(candidate);
            changed = true;
        }
    }
    changed
}

fn label_of(account: &Value) -> Option<&str> {
    account.get("label").and_then(Value::as_str)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn write_private_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("credential store has no file name"))?;
    let mut temporary_name = file_name.to_os_string();
    temporary_name.push(format!(".cruise-tmp.{}", std::process::id()));
    let temporary = path.with_file_name(temporary_name);
    let result = fs::write(&temporary, contents)
        .and_then(|()| set_private_file(&temporary))
        .and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn lock_exclusive(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    set_private_file(path)?;
    fs2::FileExt::lock_exclusive(&file)?;
    Ok(file)
}

#[cfg(test)]
mod tests;
