# cruise

**Languages:** [English](../en/README.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/README.md) · [简体中文](../zh/README.md) · [Index](README.md)


<p align="center">
  <img src="../icon.png" alt="cruise logo" width="160" height="160">
</p>

A YAML-driven coding-agent workflow orchestrator with the jcode SDK as its primary, default backend (`sdk: jcode`).

Cruise drives agents through declarative workflows: plan → approve → write tests → implement → test → review → optionally open a PR and run after-PR automation. It handles variable passing between steps, conditional branching, and loop control. Claude and OpenCode SDK backends and custom CLI commands are also supported.

> **Note:** Cruise runs on macOS, Linux, and Windows (x86-64, `x86_64-pc-windows-msvc`). Development and testing happen primarily on macOS; Linux and Windows have been verified less extensively.

## Documentation topics

All locale editions use the same topic filenames. Keep guide changes synchronized across English, Japanese, and Simplified Chinese.

- [Installation](installation.md) — prerequisites, platform installers, updates, and managed provider CLIs.
- [Usage](usage.md) — WebUI, TUI, herdr, and workflow packages.
- [CLI reference](cli-reference.md) — commands, options, and UI-language settings.
- [Sessions](sessions.md) — lifecycle, workspace modes, isolation, and parallel runs.
- [Configuration](configuration.md) — config resolution, defaults, settings, and environment variables.
- [SDK reference](sdk.md) — SDK backends, permissions, and tool restrictions.
- [Steps](steps.md) — step types, after-PR automation, and variables.
- [Workflow control](workflow-control.md) — branching, groups, and workflow composition.
- [Workflow examples](workflow-examples.md) — complete and simple sample workflows.
- [GitHub Actions](github-actions.md) — issue commands, providers, authentication, security, and troubleshooting.
- [Telemetry](telemetry.md) — OpenTelemetry configuration and exported signals.

## Development build

To test the launcher locally, build both binaries. The `cruise` launcher starts the adjacent `cruise-runtime`, so both are needed:

```sh
cargo build --bin cruise --bin cruise-runtime
```

`cargo test --all-features --test workflow_defaults_cli` also checks CLI startup with a 1 MiB main-stack budget on Unix; Windows CI exercises the native main-thread limit.

## Support

If you find cruise useful, consider [sponsoring smartcrabai](https://github.com/sponsors/smartcrabai)
to support its development and maintenance.

## License

MIT
