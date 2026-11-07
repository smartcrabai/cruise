# NEVER

Features cruise does not implement.

- **LLM-chosen step transitions.** Transitions come only from exit status (`prev.success`, `if.fail`, `quality_gates`), working-tree diffs (`if.file-changed`, `if.no-file-changes`), glob matches (`when.exists`), explicit `next`, and human `option` selections. Workflow call return labels are statically converted to explicit `next` at load time, not chosen by an LLM. `NO_CHANGES_INTENTIONAL` only waives `if.no-file-changes` for one attempt and cannot route to another step.
- **Persona, policy, or knowledge facet composition.** A prompt step is one prompt body (`prompt` or `prompt_file`) plus `model` and `env`.
- **Per-vendor provider adapters.** Backends are `sdk: jcode`, `sdk: claude`, `sdk: opencode`, and `command:`. Model switching is failure-driven fallback (`retry`, model arrays) plus user-declared `disabled_during` windows on model-array entries. These windows exclude candidates during declared unavailable times, not automatic cost routing: cruise never inspects prices or quotas to choose a model.
- **Runtime state in the project repository.** Sessions, worktrees, artifacts, logs, and history live in XDG directories. A repository holds only workflow config (`cruise.yaml`, `.cruise/*.yaml`) and `.worktreeinclude`.
- **Resident watcher.** Execution starts only from `cruise run`, the TUI or WebUI, or an `@cruise` mention on GitHub. Instruction submission via `cruise tell`, TUI/WebUI Tell, or GitHub `/tell` does not start execution; it writes only to a running run's inbox.
- **Automatic WebUI service installation.** `cruise webui start` starts a user-requested background server, not a boot-time service or a watcher that starts workflows.
- **MCP or ACP server.** cruise consumes MCP servers (`mcp_servers`) but exposes none.
