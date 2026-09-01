//! `cruise login`: sign in to a model provider for the `sdk: jcode` backend.
//!
//! Cruise drives jcode with `JCODE_HOME` pointed at its own directory
//! ([`crate::backend::jcode::jcode_home`]), so the user's `~/.jcode` -- and the
//! login their own jcode TUI depends on -- is never read or written. That
//! isolation needs a way to put credentials *into* cruise's home, which is what
//! this command is.
//!
//! It is deliberately a thin wrapper: the provider list, the OAuth flows and the
//! credential storage format are all jcode's. Cruise adds only its action menu,
//! API-key input wiring, and status presentation, then builds the invocations
//! ([`jcode_command`], shared with the backend so the `JCODE_HOME` / telemetry /
//! no-update handling is defined once) for `jcode login` / `jcode auth status` /
//! `jcode model list`.
//! No credential ever passes through a cruise config file.

use std::fmt::Write as _;
use std::io::{IsTerminal as _, Read as _, Write as _};
use std::path::Path;
use std::process::Stdio;

use crate::backend::jcode::{auth_status, jcode_command, jcode_home, resolve_binary};
use crate::cli::LoginArgs;
use crate::error::{CruiseError, Result};
use console::style;
use inquire::InquireError;

/// Environment variable used by the `--api-key` flow. A non-empty value takes
/// precedence over a hidden terminal prompt or piped stdin.
pub const API_KEY_ENV: &str = "CRUISE_LOGIN_API_KEY";

const LOGIN_ACTIONS: [&str; 4] = [
    "Sign in or configure a provider",
    "Store an API key directly",
    "View authentication status",
    "Exit",
];

#[derive(Debug, Clone, Copy)]
enum LoginAction {
    SignIn,
    ApiKey,
    Status,
    Exit,
}

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
    if let Some(provider) = args.provider.as_deref() {
        return interactive_login(&home, Some(provider));
    }
    if login_menu_is_interactive() {
        return interactive_menu(&home);
    }
    interactive_login(&home, None)
}

fn login_menu_is_interactive() -> bool {
    // The menu writes its header/status to stdout and inquire renders prompts
    // on stderr. Require all three streams to remain terminals so redirecting
    // either output stream keeps the one-shot, non-interactive behavior.
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal()
}

fn interactive_menu(home: &Path) -> Result<()> {
    print_header(home);

    loop {
        let Some(action) = prompt_action()? else {
            return Ok(());
        };

        match action {
            LoginAction::SignIn => {
                interactive_login(home, None)?;
                print_success("Provider login completed.");
            }
            LoginAction::ApiKey => {
                let Some(provider) = prompt_provider()? else {
                    continue;
                };
                let Some(key) = read_api_key_with_cancel()? else {
                    continue;
                };
                store_api_key_value(home, &provider, &key)?;
                print_success("API key stored in cruise's private jcode home.");
            }
            LoginAction::Status => print_status(home)?,
            LoginAction::Exit => return Ok(()),
        }
    }
}

fn print_header(home: &Path) {
    println!("{}", style("cruise login").cyan().bold());
    println!("{}", style("Cruise authentication").cyan().bold());
    println!("Configure providers for cruise's private jcode backend");
    println!(
        "{}",
        style(format!("Credential home: {}", home.display())).dim()
    );
    println!(
        "{}",
        style("Your personal ~/.jcode is not read or modified.").dim()
    );
    println!();
}

fn prompt_action() -> Result<Option<LoginAction>> {
    crate::platform::reclaim_terminal_foreground();
    let action = match inquire::Select::new("What would you like to do?", LOGIN_ACTIONS.to_vec())
        .with_help_message("Use ↑↓, Enter to select")
        .prompt()
    {
        Ok(action) => action,
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Ok(None);
        }
        Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
    };

    let action = match action {
        "Sign in or configure a provider" => LoginAction::SignIn,
        "Store an API key directly" => LoginAction::ApiKey,
        "View authentication status" => LoginAction::Status,
        "Exit" => LoginAction::Exit,
        _ => {
            return Err(CruiseError::Other(
                "selected login action not found".to_string(),
            ));
        }
    };
    Ok(Some(action))
}

fn prompt_provider() -> Result<Option<String>> {
    crate::platform::reclaim_terminal_foreground();
    let provider = match inquire::Text::new("Provider ID:").prompt() {
        Ok(provider) => provider,
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Ok(None);
        }
        Err(e) => return Err(CruiseError::Other(format!("input error: {e}"))),
    };
    let provider = provider.trim().to_string();
    if provider.is_empty() {
        eprintln!("{} Provider ID is required.", style("!").yellow().bold());
        return Ok(None);
    }
    Ok(Some(provider))
}

fn print_success(message: &str) {
    println!("{} {}", style("✓").green().bold(), style(message).green());
}

/// Keep jcode-provided identifiers from being interpreted as terminal
/// controls while retaining their textual value in rich terminal output.
fn display_terminal_text(value: &str) -> String {
    let mut displayed = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            let _ = write!(displayed, "\\u{{{:04x}}}", character as u32);
        } else {
            displayed.push(character);
        }
    }
    displayed
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
    store_api_key_value(home, provider, &read_api_key()?)
}

fn store_api_key_value(home: &Path, provider: &str, key: &str) -> Result<()> {
    let mut child = jcode_command(&resolve_binary(None), home)
        .arg("login")
        .arg(provider)
        .arg("--no-validate")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error(&e))?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        CruiseError::Other("could not write the API key to jcode's stdin".to_string())
    })?;
    stdin.write_all(key.as_bytes())?;
    stdin.write_all(b"\n")?;
    // Dropping the handle closes the pipe, so jcode's prompt reader sees EOF
    // after the key rather than blocking for more input.
    drop(stdin);
    let status = child.wait()?;
    exit_status_to_result(status, "jcode login")
}

/// The API key from [`API_KEY_ENV`], else an echo-less prompt on a terminal,
/// else the first line of stdin. A cancelled terminal prompt is represented by
/// `None` so the interactive menu can return to its action list.
fn read_api_key() -> Result<String> {
    read_api_key_with_cancel()?.ok_or_else(|| {
        CruiseError::Other("could not read the API key: operation cancelled".to_string())
    })
}

fn read_api_key_with_cancel() -> Result<Option<String>> {
    if let Ok(from_env) = std::env::var(API_KEY_ENV) {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }
    // On a terminal, prompt without echo rather than making the user pipe the
    // key in: `printf %s "$KEY" | ...` puts the secret in shell history.
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let key =
            match inquire::Password::new(&format!("API key (not echoed; or set {API_KEY_ENV}):"))
                .without_confirmation()
                .with_display_mode(inquire::PasswordDisplayMode::Hidden)
                .prompt()
            {
                Ok(key) => key,
                Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                    return Ok(None);
                }
                Err(e) => {
                    return Err(CruiseError::Other(format!(
                        "could not read the API key: {e}"
                    )));
                }
            };
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(CruiseError::Other("no API key entered".to_string()));
        }
        return Ok(Some(key));
    }
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let key = buf.lines().next().unwrap_or_default().trim().to_string();
    if key.is_empty() {
        return Err(CruiseError::Other(
            "the piped API key was empty".to_string(),
        ));
    }
    Ok(Some(key))
}

/// Print which providers cruise's jcode home can authenticate as, and which
/// models they offer.
fn print_status(home: &Path) -> Result<()> {
    if std::io::stdout().is_terminal() {
        return print_status_rich(home);
    }

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

fn print_status_rich(home: &Path) -> Result<()> {
    println!("{}", style("Authentication status").cyan().bold());
    println!(
        "{}",
        style(format!("Credential home: {}", home.display())).dim()
    );

    let status = auth_status(None, home, &std::collections::HashMap::new())?;
    let available = status.available();
    println!("Authenticated providers ({})", available.len());
    for provider in &available {
        println!("{} {}", style("✓").green(), display_terminal_text(provider));
    }

    if available.is_empty() {
        println!(
            "{} No authenticated providers. Run {} to sign in.",
            style("!").yellow().bold(),
            style("cruise login").cyan()
        );
        println!("Available models (0)");
        return Ok(());
    }

    let models = list_models(home)?;
    println!("Available models ({})", models.len());
    if models.is_empty() {
        println!("{} No models reported.", style("!").yellow());
    } else {
        for model in models {
            println!("{} {}", style("•").cyan(), display_terminal_text(&model));
        }
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
            v.get("models")?.as_array().map(|models| {
                models
                    .iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            })
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

    #[test]
    fn rich_status_text_escapes_terminal_controls_without_dropping_value() {
        assert_eq!(
            display_terminal_text("provider\n\u{1b}[31m"),
            r"provider\u{000a}\u{001b}[31m"
        );
    }
}
