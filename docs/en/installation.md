**Languages:** [English](../en/installation.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/installation.md) · [简体中文](../zh/installation.md) · [Index](README.md)

<a id="prerequisites"></a>
## Prerequisites

- [`gh` CLI](https://cli.github.com/) — required for GitHub worktree sessions when automatic PR creation is enabled (the default), PR-backed cleanup, or an explicitly enabled workflow merge. Use `glab` for GitLab repositories. A local worktree workflow with `create_pr: false` does not need a forge CLI, but `--repo` requires `gh`/`glab` to access and clone the repository.
- [`glab` CLI](https://gitlab.com/gitlab-org/cli) — required instead of `gh` for GitLab repositories (merge request creation, cleanup, issue publishing, and explicitly enabled native synchronous squash merge). The forge is detected from the host (`github.com` / `gitlab.com`). For self-managed hosts, set `CRUISE_FORGE=github|gitlab`; for `owner/repo`-style `--repo` locators, also set `GITLAB_HOST`. `--repo` accepts `owner/repo`, nested `group/sub/project` (with `CRUISE_FORGE=gitlab`), or a full URL. On GitLab, `{pr.number}` / `{pr.url}` refer to the merge request, and the option to post an `@cruise run` comment when publishing issues is unavailable.
- [`jcode` CLI](https://github.com/1jehuang/jcode) — required by the default SDK backend (`sdk: jcode`); sign in with `jcode login <provider>` after installing. For SDK execution, install jcode whenever an effective execution or planning candidate uses it, including SDK-qualified array entries under another workflow SDK.
- [`claude` CLI](https://code.claude.com/docs/en/quickstart) — required whenever an effective execution or planning candidate uses claude, including SDK-qualified array entries. Cruise drives it in-process. A model name's `:effort` suffix maps to `claude --effort`, so a CLI without that flag fails the step with `unknown option '--effort'`. Authentication is Claude's own and unrelated to `jcode login`.
- `opencode` CLI with the V2 API — required whenever an effective execution or planning candidate uses opencode, including SDK-qualified array entries. Cruise starts and stops it locally and uses your existing OpenCode provider configuration and authentication. V1 is not supported.
- An OpenSSH client (`ssh`) on the local `PATH` for `cruise ssh`.
- On Windows, [Git for Windows](https://gitforwindows.org/) (`git` on `PATH`) is required.
- The SSH destination needs a compatible `cruise` and the usual prerequisites for the selected workflow, such as `gh`, `jcode`, `claude`, or `opencode` when required. The workflow runs on the remote host, which owns its authentication, configuration, sessions, worktrees, and clones.
- Install a managed provider on the destination, for example with `cruise ssh <host> -- install jcode`.

<a id="installation"></a>
## Installation

<a id="windows-powershell"></a>
### Windows (PowerShell)

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.ps1 | iex"
```

<a id="macos-and-linux-shell-installer"></a>
### macOS and Linux (shell installer)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh | sh
```

The shell and PowerShell installers place `cruise` in `$CARGO_HOME/bin` (normally `~/.cargo/bin`; on Windows, `%USERPROFILE%\.cargo\bin` when `CARGO_HOME` is unset). Keep that directory on your `PATH`. Supported managed installs can use `cruise update`; see [Updating Cruise](#updating-cruise) below. To install a pinned release, replace `/releases/latest/` in the installer URL with `/releases/download/v<version>/`, for example `v0.2.15`. Keep Homebrew installations managed with Homebrew.

<a id="homebrew"></a>
### Homebrew

```sh
brew install smartcrabai/tap/cruise
```

Upgrade a Homebrew installation with:

```sh
brew upgrade smartcrabai/tap/cruise
```

<a id="updating-cruise"></a>
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
| GitHub Action, Docker image, or another external installation | Use that distribution's original update method |
| Any installation with `CRUISE_RUNTIME` set | Unset the override and run `cruise update` from the managed launcher |

“Managed” is determined by the canonical `$CARGO_HOME/bin` location, not Cargo's install ledger. The updater replaces only the runtime selection: the fixed `cruise` launcher (`cruise.exe` on Windows) remains in place; each update adds a versioned `cruise-runtime-managed-…` runtime and `.json` sidecar and switches `.cruise-runtime-active`. Older runtimes are retained. Native update does not run Cargo or change its install ledger. If you need a custom source-built runtime, rebuild and install it from source instead.

Updating Cruise does not replace the code in an already running WebUI process. After an update, use `cruise webui status` to compare its version with the current runtime and `cruise webui restart` to load the updated runtime. Restart cancels active WebUI operations, so choose a suitable time.

<a id="managed-provider-clis"></a>
### Managed provider CLIs

Install only the providers your workflows need:

```sh
cruise install jcode
cruise install claude
cruise install opencode
cruise install --status
cruise install jcode --force
```
