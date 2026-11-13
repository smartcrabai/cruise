# Cruise installation and updates

**Languages:** [English](../en/installation.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/installation.md) · [简体中文](../zh/installation.md) · [Index](README.md)

## Prerequisites

- [`gh` CLI](https://cli.github.com/) -- required for GitHub worktree sessions when automatic PR creation is enabled (the default), PR-backed cleanup, or an opted-in native workflow merge; `glab` for GitLab repositories. A local worktree workflow with `create_pr: false` does not need a forge CLI, but `--repo` still needs `gh`/`glab` to access and clone the repository.
- [`glab` CLI](https://gitlab.com/gitlab-org/cli) -- required instead of `gh` for GitLab repositories (merge request creation, cleanup, issue publishing, and opted-in native synchronous squash merge). The forge is detected from the host (`github.com` / `gitlab.com`); set `CRUISE_FORGE=github|gitlab` for self-managed hosts (and `GITLAB_HOST` for `owner/repo`-style `--repo` locators). `--repo` accepts `owner/repo`, nested `group/sub/project` (with `CRUISE_FORGE=gitlab`), or a full URL. On GitLab, `{pr.number}` / `{pr.url}` refer to the merge request, and the "post an `@cruise run` comment" option when publishing issues is unavailable.
- [`jcode` CLI](https://github.com/1jehuang/jcode) v0.88.0 or newer -- required by the default SDK backend (`sdk: jcode`); sign in with `jcode login <provider>` after installing. For SDK execution, install jcode whenever an effective execution or planning candidate uses jcode, including SDK-qualified array entries under another workflow SDK.
- [`claude` CLI](https://code.claude.com/docs/en/quickstart) -- required whenever an effective execution or planning candidate uses claude, including SDK-qualified array entries. Cruise drives it in-process. Verified against 2.1.250; a `:effort` model suffix maps to `claude --effort`, so a CLI without that flag fails the step with `unknown option '--effort'` (a permanent error, not retried). Authentication is the CLI's own, unrelated to `jcode login`.
- `opencode` CLI with the V2 API, required whenever an effective execution or planning candidate uses opencode, including SDK-qualified array entries. Cruise starts and stops it locally and uses your existing OpenCode provider configuration and authentication. V1 is not supported.
- An OpenSSH client (`ssh`) on the local `PATH` for `cruise ssh`.
- On Windows: [Git for Windows](https://gitforwindows.org/) (`git` on `PATH`). `command:` steps run through `cmd.exe /C`, so use cmd.exe syntax (or invoke `powershell -Command ...` / `bash -c ...` explicitly) rather than POSIX shell syntax. Desktop notifications and notification sounds are unavailable, but configured webhooks are sent. `cruise ssh` needs the Windows OpenSSH client, and herdr reporting is limited to what the herdr environment variables provide.
- A compatible `cruise` version and its usual workflow prerequisites on the SSH destination, including any required `gh`, `jcode`, `claude`, or `opencode` CLI for the selected workflow. The remote host performs the workflow and owns its authentication, configuration, sessions, worktrees, and clones.
- Install a managed provider on that host with, for example, `cruise ssh <host> -- install jcode`.

## Installation

### Windows (PowerShell)

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.ps1 | iex"
```

### macOS and Linux (shell installer)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh | sh
```

Or download the installer to a file and run it only if the download succeeds:

```sh
curl --proto '=https' --tlsv1.2 -fLsS --output cruise-installer.sh \
  https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh \
  && sh ./cruise-installer.sh
```

The shell and PowerShell installers place `cruise` in `$CARGO_HOME/bin` (normally `~/.cargo/bin`; on Windows, `%USERPROFILE%\.cargo\bin` when `CARGO_HOME` is unset). Keep that directory on your `PATH`. Supported managed installs can use `cruise update`; see [Updating Cruise](#updating-cruise) below. To install a pinned release, replace `/releases/latest/` in the installer URL with `/releases/download/v<version>/`, for example `v0.2.15`. Keep Homebrew installations managed with Homebrew.

### Homebrew

```sh
brew install smartcrabai/tap/cruise
```

Upgrade a Homebrew installation with:

```sh
brew upgrade smartcrabai/tap/cruise
```

### Updating Cruise

For a supported managed installation, check the version before and after updating:

```sh
cruise --version; cruise update; cruise --version
```

`cruise update` fetches the latest official stable release. It does nothing when that version is the same as or older than the installed version. Native updates are supported for these release targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`, and `x86_64-pc-windows-msvc`.

| Installation | Update method |
|---|---|
| Shell or PowerShell installer in canonical `$CARGO_HOME/bin` | `cruise update` |
| Source-built installation in canonical `$CARGO_HOME/bin` | `cruise update`; it installs the official runtime, not your local source build or its build-time changes |
| Homebrew | `brew upgrade smartcrabai/tap/cruise` |
| GitHub Action, Docker image, or another external installation | Use that distribution’s original update method |
| Any installation with `CRUISE_RUNTIME` set | Unset the override and run `cruise update` from the managed launcher |

“Managed” is determined by the canonical `$CARGO_HOME/bin` location, not Cargo’s install ledger. The updater replaces only the runtime selection: the fixed `cruise` launcher (`cruise.exe` on Windows) remains in place; each update adds a versioned `cruise-runtime-managed-…` runtime and `.json` sidecar and switches `.cruise-runtime-active`. Older runtimes are retained. Native update does not run Cargo or change its install ledger. If you need a custom source-built runtime, rebuild and install it from source instead.

Published versions without the updater—including v0.2.16—cannot update themselves. An updater-capable build can select v0.2.16, but that published runtime cannot perform a subsequent native update. Bootstrap by installing an updater-enabled release with its platform installer, or rebuilding/installing that source version; use `cruise update` for later updates only when the installed runtime includes the updater.

The downloaded release is checked over HTTPS against its release manifest and SHA-256 checksum. The checksum is distributed by the same release publisher; this is not an independent digital signature.

`cruise update --status` lists recorded transactions for all installations in sorted order. It is read-only: it does not contact the network, acquire the update lock, repair state, or prove which runtime is currently active. The transaction file is `$XDG_STATE_HOME/cruise/installations/<installation-id>/update-transaction.json` (default `~/.local/state/cruise/installations/<installation-id>/update-transaction.json`; on Windows, `%LOCALAPPDATA%\cruise\state\installations\<installation-id>\update-transaction.json` when `XDG_STATE_HOME` is unset). A failed transaction keeps its phase and error. Retry with `cruise update`; it reconciles an interrupted transaction before starting another download. Pre-activation failures leave the old selection in place. If the post-activation launcher probe fails, Cruise restores the previous selection; if recovery cannot prove the selected runtime, leave the record and use the reported error to guide manual recovery rather than deleting transaction files.

If Cruise reports that an update was applied but transaction completion or cleanup warned, the new runtime is already active; it was not rolled back. Inspect `cruise update --status`, then retry `cruise update` to recover or clean the recorded transaction. A status phase is metadata, not proof of activation; check the running version with `cruise --version`.

Updating Cruise does not replace the code in an already running WebUI process. After an update, use `cruise webui status` to compare its version with the current runtime and `cruise webui restart` to load the updated runtime. Restart cancels active WebUI operations, so choose a suitable time.

Versioned runtimes can coexist in `$CARGO_HOME/bin` (on Windows, `%USERPROFILE%\.cargo\bin` by default). To pin one already-installed version, verify that exact versioned sibling with `--version`, ensure its adjacent `.json` sidecar belongs to the current fixed launcher/runtime baseline, then set `.cruise-runtime-active` to that runtime’s exact basename. Do not delete the other runtime files. The baseline is the fixed launcher/runtime size-and-mtime stamp: a reinstall that changes it invalidates the overlay, while a same-size, same-mtime replacement is not detected. Legacy pointers or pinning an older launcher may still require manual retirement. To retire the pointer, first verify the fixed sibling directly; archive and move the pointer only then:

```sh
# macOS / Linux
bin="${CARGO_HOME:-$HOME/.cargo}/bin"
archive="$bin/.cruise-runtime-active.archive"
retired="$bin/.cruise-runtime-active.retired"
test ! -e "$archive" && test ! -e "$retired" &&
  "$bin/cruise-runtime" --version &&
  cp -n "$bin/.cruise-runtime-active" "$archive" &&
  mv -n "$bin/.cruise-runtime-active" "$retired" &&
  "$bin/cruise" --version
```

```powershell
# Windows
$bin = if ($env:CARGO_HOME) { Join-Path $env:CARGO_HOME 'bin' } else { Join-Path $HOME '.cargo\bin' }
$runtime = Join-Path $bin 'cruise-runtime.exe'
$pointer = Join-Path $bin '.cruise-runtime-active'
$launcher = Join-Path $bin 'cruise.exe'
$archive = "$pointer.archive"
$retired = "$pointer.retired"
if ((Test-Path -LiteralPath $archive) -or (Test-Path -LiteralPath $retired)) { throw 'Choose unused archive and retired paths.' }
& $runtime --version
if (-not $? -or $LASTEXITCODE -ne 0) { throw 'The fixed sibling runtime did not pass --version; pointer was not moved.' }
Copy-Item -LiteralPath $pointer -Destination $archive -ErrorAction Stop
Move-Item -LiteralPath $pointer -Destination $retired -ErrorAction Stop
& $launcher --version
```

The first `--version` above must succeed before moving the pointer. These commands only archive and move the pointer file; never delete a runtime or sidecar. Choose unused archive/retired names if those files already exist.

### Managed provider CLIs

Install only the providers your workflows need:

```sh
cruise install jcode
cruise install claude
cruise install opencode
cruise install --status
cruise install jcode --force
```

This build pins jcode **0.99.1**, claude **2.1.250**, and opencode **2.1.3**. Downloads are checked against digests pinned in Cruise and the extracted binary's version is checked before publication. No vendor installer, npm CLI, or shell configuration changes are required. Managed files live under `$XDG_DATA_HOME/cruise/providers/<provider>/` (normally `~/.local/share/cruise/providers/`). Each installation uses a distinct version-prefixed directory, with the active relative path recorded in `install.json`. Previous directories are retained so concurrent readers can continue using their resolved paths. Availability depends on platform. Authentication remains the provider's responsibility.

When jcode is installed only by Cruise, find its active `relative_path` in `providers/jcode/install.json` under the Cruise data directory, then invoke that binary with `login <provider>` or `auth status`. On Windows, the default data directory is `%LOCALAPPDATA%\cruise\data` when `XDG_DATA_HOME` is unset and `LOCALAPPDATA` is set. If `LOCALAPPDATA` is unavailable, the home-directory fallback applies. Quote paths containing spaces.

Resolution uses a nonempty process environment override first, then a valid managed installation matching the current pin, then the existing PATH lookup. The overrides are `CRUISE_JCODE_CLI_PATH`, `CRUISE_CLAUDE_CLI_PATH`, and `CRUISE_OPENCODE_CLI_PATH`. An invalid explicit override is an error, not a fallback. These are process environment variables, not workflow `env:` settings. A missing, stale, or damaged managed installation falls back to PATH. Running workflows, planning, TUI, and WebUI never download or install providers.

`--status` is read-only: it does not access the network, execute provider binaries, or create files. Reinstall with `--force` when needed. Rerun `cruise install <provider>` after a Cruise update changes its pin. `cruise update` updates Cruise itself, not providers. For managed claude and opencode, Cruise disables their auto-updaters only when the respective environment variable is not already set. PATH and override installations retain their existing behavior. The GitHub Action continues to manage its own jcode installation and version input separately.
