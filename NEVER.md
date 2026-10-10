# NEVER

Features cruise does not implement.

- **LLM-chosen step transitions.** Transitions come only from exit status (`prev.success`, `if.fail`), working-tree diffs (`if.file-changed`, `if.no-file-changes`), glob matches (`when.exists`), explicit `next`, and human `option` selections. `NO_CHANGES_INTENTIONAL` only waives `if.no-file-changes` for one attempt and cannot route to another step.
- **Persona, policy, or knowledge facet composition.** A prompt step is one prompt body (`prompt` or `prompt_file`) plus `model` and `env`.
- **Per-vendor provider adapters.** Backends are `sdk: jcode`, `sdk: claude`, and `command:`. Model switching is failure-driven fallback (`retry`, model arrays), not cost-based routing.
- **Runtime state in the project repository.** Sessions, worktrees, artifacts, logs, and history live in XDG directories. A repository holds only workflow config (`cruise.yaml`, `.cruise/*.yaml`) and `.worktreeinclude`.
- **Resident watcher.** Execution starts only from `cruise run`, the TUI or WebUI, or an `@cruise` mention on GitHub.
- **MCP or ACP server.** cruise consumes MCP servers (`mcp_servers`) but exposes none.
