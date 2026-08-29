//! `cruise login`: sign in to a model provider for the `sdk: jcode` backend.
//!
//! Cruise drives jcode with `JCODE_HOME` pointed at its own directory
//! ([`crate::backend::jcode::jcode_home`]), so the user's `~/.jcode` -- and the
//! login their own jcode TUI depends on -- is never read or written. That
//! isolation needs a way to put credentials *into* cruise's home, which is what
//! this command is.
//!
//! It is deliberately a thin wrapper: the provider list, the OAuth flows and the
//! credential storage format are all jcode's, and cruise only builds the
//! invocation ([`jcode_command`], shared with the backend so the `JCODE_HOME` /
//! telemetry / no-update handling is defined once) and runs `jcode login` /
//! `jcode auth status` / `jcode model list`. No credential ever passes through a
//! cruise config file.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::Stdio;

use crate::backend::jcode::{auth_status, jcode_command, jcode_home, resolve_binary};
use crate::cli::LoginArgs;
use crate::error::{CruiseError, Result};

/// Environment variable read by `--api-key` when stdin is not being piped in.
pub const API_KEY_ENV: &str = "CRUISE_LOGIN_API_KEY";

/// Run `cruise login`.
///
/// # Errors
///
/// Returns an error if cruise's jcode home cannot be created, if `jcode` cannot
/// be run, if `--api-key` is used without a provider or without a key, or if
/// jcode exits non-zero.
pub fn run(args: &LoginArgs) -> Result<()> {
    let home = jcode_home()?;
    std::fs::create_dir_all(&home)?;
    if args.status {
        return print_status(&home);
    }
    if args.api_key {
        let provider = args.provider.as_deref().ok_or_else(|| {
            CruiseError::Other(
                "`cruise login --api-key` needs a provider, e.g. \
                 `cruise login --api-key anthropic-api`"
                    .to_string(),
            )
        })?;
        return store_api_key(&home, provider);
    }
    interactive_login(&home, args.provider.as_deref())
}

/// Hand control to `jcode login`, with cruise's home in the environment.
///
/// stdio is inherited so jcode's own provider picker and OAuth prompts work
/// exactly as they do outside cruise.
fn interactive_login(home: &Path, provider: Option<&str>) -> Result<()> {
    let mut command = jcode_command(&resolve_binary(None), home);
    command.arg("login");
    if let Some(provider) = provider {
        command.arg(provider);
    }
    let status = command
        .stdin(Stdio::inherit())
        .status()
        .map_err(|e| spawn_error(&e))?;
    exit_status_to_result(status, "jcode login")
}

/// Feed an API key to `jcode login <provider>` on stdin.
///
/// jcode prompts for the key on stdin for its API-key providers (its own
/// `--api-key` flag is for OpenAI-compatible profiles only), so a piped key
/// drives that same code path -- and therefore the same storage location and
/// file mode -- without cruise knowing the format. The key is taken from
/// [`API_KEY_ENV`] when set, otherwise prompted for or read from cruise's own
/// stdin, so it never appears in a process argument list.
///
/// `--no-validate` saves the key without jcode's post-login live provider
/// check: cruise has already closed stdin by then, so a validation failure that
/// re-prompts would have nothing to read.
fn store_api_key(home: &Path, provider: &str) -> Result<()> {
    let key = read_api_key()?;
    let mut child = jcode_command(&resolve_binary(None), home)
        .arg("login")
        .arg(provider)
        .arg("--no-validate")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error(&e))?;
    {
        let stdin = child.stdin.as_mut().ok_or_else(|| {
            CruiseError::Other("could not write the API key to jcode's stdin".to_string())
        })?;
        stdin.write_all(key.as_bytes())?;
        stdin.write_all(b"\n")?;
    }
    // Dropping the handle closes the pipe, so jcode's prompt reader sees EOF
    // after the key rather than blocking for more input.
    drop(child.stdin.take());
    let status = child.wait()?;
    exit_status_to_result(status, "jcode login")
}

/// The API key from [`API_KEY_ENV`], else an echo-less prompt on a terminal,
/// else the first line of stdin.
fn read_api_key() -> Result<String> {
    if let Ok(from_env) = std::env::var(API_KEY_ENV) {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    // On a terminal, prompt without echo rather than making the user pipe the
    // key in: `printf %s "$KEY" | ...` puts the secret in shell history.
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let key = inquire::Password::new(&format!("API key (not echoed; or set {API_KEY_ENV}):"))
            .without_confirmation()
            .with_display_mode(inquire::PasswordDisplayMode::Hidden)
            .prompt()
            .map_err(|e| CruiseError::Other(format!("could not read the API key: {e}")))?;
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(CruiseError::Other("no API key entered".to_string()));
        }
        return Ok(key);
    }
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let key = buf.lines().next().unwrap_or_default().trim().to_string();
    if key.is_empty() {
        return Err(CruiseError::Other(
            "the piped API key was empty".to_string(),
        ));
    }
    Ok(key)
}

/// Print which providers cruise's jcode home can authenticate as, and which
/// models they offer.
fn print_status(home: &Path) -> Result<()> {
    println!("cruise jcode home: {}", home.display());
    let status = auth_status(None, home, &std::collections::HashMap::new())?;
    let available = status.available();
    if available.is_empty() {
        // jcode's model listing runs its interactive provider picker when the
        // home has no credentials, so there is nothing to enumerate yet.
        println!("providers: none authenticated -- run `cruise login` to sign in");
        return Ok(());
    }
    println!("providers: {}", available.join(", "));
    let models = list_models(home)?;
    if models.is_empty() {
        println!("models: none reported");
    } else {
        println!("models: {}", models.join(", "));
    }
    Ok(())
}

/// The model ids `jcode run --model` accepts in cruise's home.
///
/// `--json` is what makes this parseable: the human output is a decorated table
/// whose header and separator lines are not model ids. A non-zero exit is an
/// error rather than an empty list, so a failing `jcode` cannot masquerade as a
/// home with no models.
fn list_models(home: &Path) -> Result<Vec<String>> {
    let output = jcode_command(&resolve_binary(None), home)
        .args(["model", "list", "--json"])
        .output()
        .map_err(|e| spawn_error(&e))?;
    exit_status_to_result(output.status, "jcode model list").map_err(|e| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        match stderr.lines().find(|l| !l.trim().is_empty()) {
            Some(first) => CruiseError::Other(format!("{e}: {}", first.trim())),
            None => e,
        }
    })?;
    let models = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|v| {
            Some(
                v.get("models")?
                    .as_array()?
                    .iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect(),
            )
        })
        .ok_or_else(|| {
            CruiseError::Other(
                "could not read the model list from `jcode model list --json`".to_string(),
            )
        })?;
    Ok(models)
}

fn spawn_error(e: &std::io::Error) -> CruiseError {
    CruiseError::Other(format!(
        "`cruise login` needs the `jcode` CLI on PATH, but running it failed: {e}"
    ))
}

fn exit_status_to_result(status: std::process::ExitStatus, what: &str) -> Result<()> {
    if status.success() {
        return Ok(());
    }
    Err(CruiseError::Other(match status.code() {
        Some(code) => format!("{what} exited with status {code}"),
        None => format!("{what} was terminated by a signal"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_without_provider_is_rejected_with_an_example() {
        let _guard = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let err = run(&LoginArgs {
            provider: None,
            api_key: true,
            status: false,
        })
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
        assert!(err.contains("needs a provider"), "got {err}");
        assert!(err.contains("cruise login --api-key"), "got {err}");
    }

    /// The key must come from the environment or a pipe, never a CLI argument
    /// (which would leak it into the process list).
    #[test]
    fn read_api_key_prefers_the_environment_variable() {
        let _guard = crate::test_support::lock_process();
        let restore = std::env::var_os(API_KEY_ENV);
        // SAFETY: `lock_process` serializes env mutation across the test binary.
        unsafe { std::env::set_var(API_KEY_ENV, "  key-from-env \n") };
        let key = read_api_key();
        match restore {
            // SAFETY: same lock as above.
            Some(value) => unsafe { std::env::set_var(API_KEY_ENV, value) },
            // SAFETY: same lock as above.
            None => unsafe { std::env::remove_var(API_KEY_ENV) },
        }
        assert_eq!(key.unwrap_or_else(|e| panic!("{e:?}")), "key-from-env");
    }

    /// `--status` must resolve cruise's own home, never the user's `~/.jcode`.
    #[test]
    fn status_reports_the_cruise_owned_jcode_home() {
        let _guard = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let home = jcode_home().unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            home.starts_with(tmp.path()),
            "{} should live under the fake home {}",
            home.display(),
            tmp.path().display()
        );
        assert!(
            !home.to_string_lossy().contains("/.jcode"),
            "{} must not be the user's jcode home",
            home.display()
        );
    }
}
