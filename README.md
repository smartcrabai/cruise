# cruise

<p align="center">
  <img src="docs/icon.png" alt="cruise logo" width="160" height="160">
</p>

A CLI tool that orchestrates coding agent workflows defined in a YAML config file.

Cruise wraps CLI coding agents such as `claude -p` and drives them through a declarative workflow: plan → approve → write tests → implement → test → review → optionally open a PR and run after-PR automation. It handles variable passing between steps, conditional branching, and loop control.

> **Note:** Cruise runs on macOS, Linux, and Windows (x86-64, `x86_64-pc-windows-msvc`). Development and testing happen primarily on macOS; Linux and Windows have been verified less extensively.

**Documentation:** [English guides](docs/en/README.md)
<!-- sakoku-ignore-next-line -->
[日本語](docs/ja/README.md) · [简体中文](docs/zh/README.md)

## Install

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.ps1 | iex"
```

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh | sh
```

See the [installation guide](docs/en/installation.md) for Homebrew, prerequisites, and updates.

## Quickstart

The default workflow uses `sdk: jcode`: authenticate a provider (`jcode login <provider>`) and configure an effective Cruise `model` in your workflow or `.cruise/default.yaml` first. Every actual jcode prompt needs that model; planning uses `plan_model`, then falls back to `model`. jcode's configured default and `JCODE_MODEL` do not substitute.

```sh
cruise plan "Add retry logic to the uploader"
cruise run
```

Browse the [English topic index](docs/en/README.md) for CLI, session, SDK, workflow, GitHub Actions, and telemetry guides.

Development, support, and license: [development guide](docs/en/README.md#development-build) · [support](docs/en/README.md#support) · [license](docs/en/README.md#license) · [LICENSE](LICENSE).
