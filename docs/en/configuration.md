# Configuration guide

**Languages:** [English](../en/configuration.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/configuration.md) · [简体中文](../zh/configuration.md) · [Index](README.md)

<a id="config-file-resolution"></a>
## Workflow config file resolution

cruise resolves the workflow config as follows:

1. **`-c/--config` flag** -- highest priority. The specified file must exist or cruise exits with an error. No prompt is shown. `-c __builtin__` selects the built-in `tdd` workflow, equivalent to `-c builtin:tdd`; `-c builtin:<name>` selects a named catalog workflow. The old selectors `builtin:default`, `builtin:simple`, and `builtin:review` are unsupported. The fixed catalog is:

   | Name | Description |
   |------|-------------|
   | `tdd` | Test-first implementation followed by a parallel analyze → consolidate → apply verification group and the PR workflow |
   | `refactor` | Diff-first code, test, and structure improvements with verification |
   | `add-tests` | Audit behavior and user-flow coverage, add meaningful tests and CI, then independently verify and repair gaps |
   | `add-formal-specs` | Add executable Quint, Alloy, and Gherkin specs with CI; fix real spec/source defects |
   | `research` | Bounded, read-only investigation with source-backed evidence, independent review, and a cited report |
   | `security` | Scoped security inventory and parallel audits, triage, safe fixes for confirmed defects, and independent verification |
   | `pr-review-fix` | Address eligible review threads on an existing GitHub PR head branch |

   `refactor`, `add-tests`, `add-formal-specs`, and `security` set `force_exec: true` by default: local planning runs directly in the current directory without an outer plan/session, worktree, or automatic PR. Pass `--no-force-exec` to use the normal plan/session path instead.

   `cruise workflow list` prints the catalog. To customize a built-in, use `cruise workflow eject <name> [--to user|project]`, for example `cruise workflow eject tdd --to project`. Ejection accepts catalog names only and never overwrites an existing file.

   `refactor` starts from the repository's existing staged, unstaged, and untracked changes plus task-branch changes against a trustworthy base/merge-base when available. It limits changes to that diff and necessary callers/dependencies; with no diff, it scopes to the whole repository. Existing work is preserved and unrelated edits are not reset, stashed, or reverted.

   The research workflow and security inventory/audit/triage phases require SDK permissions for read-only work. When neither `command:` nor `sdk:` remains set after workflow and applicable user/project defaults are resolved, Cruise uses its default `sdk: jcode`; a `command:` backend cannot be confined to read-only mode and is rejected for restricted-permission steps.

   Interactive selectors (CLI, TUI, WebUI) organize built-ins and other candidates into categories. Non-interactive resolution with no config file uses `tdd`.

   `create_pr` is a workflow-only boolean that defaults to `true`. Setting it to `false` skips Cruise's automatic commit, push, PR creation, `after-pr`, and PR-based cleanup while retaining the session, artifacts, and worktree. It is rejected in user/project `default.yaml` and is not a CLI option.

2. **`CRUISE_CONFIG` environment variable** -- if set, used directly (error if the file does not exist). No prompt is shown.
3. Otherwise, cruise collects every candidate from the following locations and presents them as choices:
   - `./cruise.yaml` -> `./cruise.yml` -> `./.cruise.yaml` -> `./.cruise.yml` (current directory)
   - `./.cruise/*.yaml` / `*.yml` (current directory), sorted by filename; the top-level `./.cruise/default.yaml` is excluded
   - `$XDG_CONFIG_HOME/cruise/workflows/*.yaml` / `*.yml` and members of valid locked bundle directories (default: `~/.config/cruise/workflows/`), sorted by relative path

   When stdin and stdout are both TTYs, candidates and built-in workflows are presented in a categorized interactive selector. In non-interactive contexts (piped stdin, scripts) the highest-priority candidate is taken automatically without a prompt.
4. **No config file found** -- an interactive terminal offers built-in workflows in the selector; non-interactive resolution selects the embedded `tdd` workflow (`builtin/tdd.yaml` in the source tree) automatically. No config file is required.

Interactive selectors use the configured `workflow_categories` tree, or, when it is empty or absent, the defaults: Development (`tdd`, `refactor`), Quality (`add-tests`, `add-formal-specs`), Research (`research`), and Security (`security`). A nonempty user tree replaces those defaults rather than merging with them. Categories and workflow entries each retain their declaration order; CLI submenus display subcategories before workflow entries. A workflow can appear in multiple categories.

Uncategorized user and built-in workflows appear in `Others` or the configured `others_category_name`. Setting `show_others_category: false` hides them. Categories affect interactive display only, not explicit selection or non-interactive resolution priority.

<a id="workflow-defaults-defaultyaml"></a>
### Workflow defaults (`default.yaml`)

`$XDG_CONFIG_HOME/cruise/default.yaml` supplies user defaults, and `./.cruise/default.yaml` supplies project defaults. When `XDG_CONFIG_HOME` is unset, the path defaults to `~/.config/cruise/default.yaml` on macOS/Linux, and to `%APPDATA%\cruise\default.yaml` on Windows (`~/.config/cruise/default.yaml` if `APPDATA` is unset). These files provide defaults for fields a workflow leaves unset and cannot be selected as workflows (`-c` or `CRUISE_CONFIG` pointing at them is an error).

Precedence, highest first where applicable: per-invocation CLI flags (including `-c` for workflow selection) > configuration environment overrides > workflow file > `./.cruise/default.yaml` > `$XDG_CONFIG_HOME/cruise/default.yaml` > application model defaults (`config.json` or `config.yaml`) > built-in defaults. Step-level model overrides still take precedence for ordinary prompts.

When an actual jcode prompt has no effective model, Cruise creates `$XDG_CONFIG_HOME/cruise/default.yaml` (or the platform's normal user config path) with `model: null` and stops before starting jcode. Edit that user-level file to set a real `provider/model[:effort]` reference or bare model. For a new session, you can instead set `model` in `config.json`, `config.yaml`, the workflow, or `CRUISE_MODEL`; planning calls can use `plan_model` / `CRUISE_PLAN_MODEL`.

After updating settings, retry the command that initiated the request: use `cruise run <SESSION>` for a failed run session, or rerun `cruise plan` or `cruise exec` for those requests. File-backed sessions reload defaults, but snapshot sessions do not reapply later defaults or environment changes; start a new snapshot session to use updated settings.

- `env` and `mcp_servers` merge per key (higher layer wins).
- `command` and `sdk` form one pair: if a layer sets either, the lower layers' pair is ignored.
- Other fields (`model`, `plan_model`, `subagent_model`, `max_retries`, `interactive_planning`, `languages`, `cleanup_after_pr`, `force_exec`, `computer_use`, `permission`, `disallowed_tools`, `retry`, `formal_verify`) are taken from the highest layer that sets them. Restriction lists replace across layers; explicit workflow `disallowed_tools: []` clears inherited defaults.
- Worktree settings `base_branch` and `auto_fetch` are defaults-only keys, with project values overriding user values. `auto_fetch: true` requires a nonempty `base_branch` after merging.
- If defaults specify a backend (`command` / `sdk`) different from the workflow's, validation fails when they fill an unset backend-specific field (`model`, `retry`, `mcp_servers`, etc.). Set the field in the workflow or remove it from `default.yaml`.

`pre_steps:` defines command-only steps (`command`, `env`, `timeout`, `skip`, `when`) inserted at the start of the workflow's `steps` as `pre/<name>`. They do not run in planning or `after-pr`. User steps come first, then project steps; a project step with the same name replaces the user one in place.

```yaml
# ./.cruise/default.yaml
pre_steps:
  mise-trust:
    command: mise trust || true
  nix:
    command: nix develop --command true
```

<a id="config-file-reference"></a>
## Config File Reference

<a id="basic-structure"></a>
### Basic Structure

`formal_verify.model_check_timeout_seconds` sets the per-command Quint model-check and Alloy timeout in seconds. It accepts an integer from 1 through 86400 and defaults to 900. It may be set in workflow YAML or layered `default.yaml`:

```yaml
formal_verify:
  model_check_timeout_seconds: 900
```

```yaml
command:                   # LLM invocation command (mutually exclusive with `sdk`)
  - claude
  - --model
  - "{model}"
  - -p

# sdk: jcode              # Use jcode-sdk (the fallback only when neither
                          # `command` nor `sdk` remains set after applicable
                          # `default.yaml` layers; see SDK Mode)
# sdk: claude             # Use claude-agent-sdk to drive the claude CLI

description: |             # one-line summary shown next to the filename in selectors (optional)
  Team-shared review-heavy flow with auto-PR.

model: sonnet             # default model for all prompt steps (optional)
# In SDK mode, arrays use the first model, then fall back in order.
# model: [sonnet, opus, gpt-5.5]
plan_model: opus          # model used for the built-in plan step (optional)
# subagent_model: haiku   # SDK-only: one subagent model, string or {sdk: claude, model: haiku}
languages:                # prompt languages (optional; defaults to English)
  pr: English             # language for auto-generated PR title/body
  plan: English           # language used by built-in planning prompts
# commit_message_template: "{title} [{session_id}]" # host-created commit message
# pr_body_template: "## Summary\n{body}"            # generated PR body wrapper
# force_exec: false       # execute direct plan entry points in place (use --no-force-exec to opt out)
# permission: full        # prompt permission: read-only | edit | full (default full)
# disallowed_tools: [web, subagent, "shell:npm publish"] # SDK prompt deny list
# computer_use: false     # jcode only, macOS: let prompts control the desktop (macos_computer_use)
# Deprecated compatibility fields: pr_language and plan_language

env:                      # environment variables applied to all steps (optional)
  API_KEY: sk-...
  PROJECT: myproject

# MCP servers are available to SDK runs only; omit this block for command mode.
# mcp_servers:
#   local_search:
#     command: npx
#     args: ["-y", "@example/search"]

flags:                    # workflow flags (optional; absent values default off, see Workflow flags)
  # run_review: {}

groups:                   # step group definitions (optional)
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr

steps:
  step_name:
    # step configuration
    # flag: run_review     # run only when the declared flag is on (optional)

after-pr:                # optional: steps that run automatically after PR creation
  step_name:
    # step configuration (same format as `steps`)
```

Unknown-key handling differs by level: individual step maps (in `steps`, `after-pr`, a group's `steps`, and parallel children) and the `retry` block reject unknown fields with a parse error, which is what turns a typo or a removed field such as `fail-if-no-file-changes` into an immediate config error. Unknown **top-level** keys are ignored silently, so a misspelled `plan_model` or `max_retries` at the top of the file is accepted and simply has no effect.

<a id="dynamic-model-selection"></a>
### Dynamic Model Selection

When the `command` array contains a `{model}` placeholder, cruise resolves it at runtime based on the effective model for each step:

- **Model specified** (via top-level `model` or step-level `model`): replaces `{model}` with the model name.
- **No model specified**: removes the `{model}` argument and its immediately-preceding `--model` flag automatically.

A step-level `model` field overrides the top-level `model` default for that step only.

The same brace escaping used for template variables (see [Variable Reference](steps.md#variable-reference)) applies here: `{{model}}` is the literal string `{model}`, not the placeholder. Any other unescaped `{name}`, an empty `{}`, an unclosed `{`, or a lone `}` is a template syntax error.

```yaml
command:
  - claude
  - --model
  - "{model}"      # replaced at runtime, or --model/{model} pair is stripped if no model
  - -p

model: sonnet      # default; steps without model: use this

steps:
  planning:
    model: opus    # overrides the default for this step only
    prompt: "Create a plan for: {input}"
```

<a id="prompt-languages"></a>
### Prompt Languages

UI display language is independent of prompt languages. Configure it with application `uiLanguage`, `CRUISE_UI_LANGUAGE`, or the TUI language picker; a WebUI browser can override the server default with its own language selector. These settings do not change `languages.pr` or `languages.plan`.

`languages.pr` controls the language for auto-generated PR titles and bodies, and `languages.plan` controls the language for Cruise's built-in planning prompts. `CRUISE_LANGUAGE_PR` and `CRUISE_LANGUAGE_PLAN` override the corresponding YAML values; blank environment values are ignored. Otherwise, the first non-empty of `LC_ALL`, `LC_MESSAGES`, `LANG`, and `LANGUAGE` is mapped once (an unsupported value does not fall through), then `English`.

```yaml
languages:
  pr: Japanese             # PR title/body will be generated in Japanese
  plan: Japanese           # generated/updated plans and plan answers will be in Japanese
```

The effective values are available to built-in templates as `{pr.language}` and `{plan.language}`.

<a id="commit-and-pr-templates"></a>
### Commit and PR Templates

Optional top-level string keys `commit_message_template` and `pr_body_template` customize the host-created commit and PR body in the worktree PR creation path. Set them in the workflow or in user `<config_dir>/default.yaml` or project `.cruise/default.yaml`. Precedence is workflow > project defaults > user defaults > unset. They are not CLI options or environment overrides.

```yaml
commit_message_template: "{title} [{session_id}]"
pr_body_template: |
  ## Summary
  {body}
```

| Template | Allowed variables |
|----------|-------------------|
| `commit_message_template` | `{title}`, `{issue}`, `{session_id}`, `{branch}` |
| `pr_body_template` | `{title}`, `{body}`, `{issue}`, `{session_id}`, `{branch}` |

`{title}` is the LLM-generated PR title, `{body}` is the LLM-generated PR body, `{issue}` currently always expands to an empty string, `{session_id}` is the session ID, and `{branch}` is the worktree branch name. These templates do not automatically add issue-closing text.

Unknown variables and malformed syntax are rejected at workflow load/compile time. Escape literal braces as `{{` and `}}`.

When both templates are unset, existing behavior is unchanged: the commit uses the generated PR title or existing input fallback, and an empty PR title uses `--fill`. These settings do not affect current-branch runs, `create_pr: false`, agent-created commits, or squash-merge messages.

<a id="session-title-generation"></a>
### Session Title Generation

After plan approval, cruise generates a concise session title (up to 80 characters) shown in `cruise list` and the WebUI sidebar instead of the raw task input.
 
<a id="environment-variables"></a>
### Environment Variables

`CRUISE_JCODE_CLI_PATH`, `CRUISE_CLAUDE_CLI_PATH`, and `CRUISE_OPENCODE_CLI_PATH` select provider executables from the Cruise process environment. They are not workflow configuration keys and cannot be supplied through workflow `env:`.

Environment variables can be set at workflow and step level. Step-level values override workflow-level values for that step only. Parallel children add a third level: workflow < parallel block < child. Values support template variable substitution; each child resolves its own values against the variables from before the block.

Process-level configuration overrides apply after workflow and defaults resolution. Supported names and value types are:

| Variable | Configuration key | Value |
|---|---|---|
| `CRUISE_MODEL` | `model` | model |
| `CRUISE_PLAN_MODEL` | `plan_model` | model |
| `CRUISE_SUBAGENT_MODEL` | `subagent_model` | model entry |
| `CRUISE_SDK` | `sdk` | text |
| `CRUISE_COMMAND` | `command` | JSON array |
| `CRUISE_MAX_RETRIES` | `max_retries` | integer |
| `CRUISE_INTERACTIVE_PLANNING` | `interactive_planning` | boolean |
| `CRUISE_CLEANUP_AFTER_PR` | `cleanup_after_pr` | boolean |
| `CRUISE_FORCE_EXEC` | `force_exec` | boolean |
| `CRUISE_COMPUTER_USE` | `computer_use` | boolean |
| `CRUISE_PERMISSION` | `permission` | text: `read-only`, `edit`, or `full` |
| `CRUISE_DISALLOWED_TOOLS` | `disallowed_tools` | JSON array |
| `CRUISE_COMMIT_MESSAGE_TEMPLATE` | `commit_message_template` | text |
| `CRUISE_PR_BODY_TEMPLATE` | `pr_body_template` | text |
| `CRUISE_RETRY` | `retry` | JSON object or `null` |
| `CRUISE_RETRY_BASE_DELAY_MS` | `retry.base_delay_ms` | integer |
| `CRUISE_RETRY_MAX_DELAY_MS` | `retry.max_delay_ms` | integer |
| `CRUISE_RETRY_MODEL_FALLBACK` | `retry.model_fallback` | boolean |
| `CRUISE_RETRY_FALLBACK_CHAINS` | `retry.fallback_chains` | JSON object |
| `CRUISE_LANGUAGES` | `languages` | JSON object |
| `CRUISE_LANGUAGE_PR` | `languages.pr` | text |
| `CRUISE_LANGUAGE_PLAN` | `languages.plan` | text |
| `CRUISE_RUN_ALL_PARALLELISM` | app `run_all_parallelism` | integer, at least 1 |

Names normally use `CRUISE_` plus the uppercase key path, replacing dots and hyphens with underscores. The language child names above are the only supported spellings. Values are trimmed and whitespace-only values are ignored. Booleans accept only `true`, `false`, `1`, or `0`; integers are unsigned decimal digits without a sign. Text is literal. Model values are literal single models unless they start with `[` or `{`, in which case they are parsed as JSON and must match the field's normal model shape. In particular, `CRUISE_MODEL` and `CRUISE_PLAN_MODEL` now accept JSON arrays: values beginning with `[` or `{` were previously treated as literal model names. Arrays with fallback entries enable model fallback as in YAML. Other composite values require JSON, not YAML flow syntax. Use JSON `null` for an optional value or `[]` for an empty list where supported, not a blank value. Invalid values produce an error naming the variable and still undergo normal configuration validation.

Parent objects are applied before child overrides, so `CRUISE_RETRY_MAX_DELAY_MS` overrides the corresponding field in `CRUISE_RETRY`, and `CRUISE_LANGUAGE_PR` overrides `CRUISE_LANGUAGES`. Setting `CRUISE_SDK` clears a configured command, while `CRUISE_COMMAND` clears a configured SDK. Setting both nonblank variables is an error. For example:

```sh
export CRUISE_MODEL='["provider/primary", "provider/fallback"]'
export CRUISE_RETRY='{"base_delay_ms":1000}'
export CRUISE_RETRY_MAX_DELAY_MS=5000
```

`CRUISE_RUN_ALL_PARALLELISM` overrides the app setting for TUI/WebUI run-all operations and effective config display, but is not saved to the config file. It does not change CLI `run --all`, which uses `--parallelism`.

`CRUISE_UI_LANGUAGE` controls the process UI language, not a workflow field or generated prompt language. See [`cruise config`](cli-reference.md#cruise-config) for supported tags and precedence.

`CRUISE_COMMIT_COAUTHOR_NAME` and `CRUISE_COMMIT_COAUTHOR_EMAIL` add a `Co-authored-by:` trailer to the commits cruise creates for a PR. Both must be set and non-blank, and a name containing `<`, `>`, or a line break -- or an invalid address -- disables the trailer instead of failing the commit.

`CRUISE_SKIP_AGENT_AUTH_CHECK=1` bypasses the SDK startup OAuth expiration check described under SDK Mode. It is a process-only runtime switch, not a workflow-config override. Only the exact value `1` skips the check.

`CRUISE_USAGE_RECORDING` is a process-only runtime switch. Set it to `0` or `false` to disable local usage recording; `1`, `true`, or an unset value enables it. Values are trimmed and case-insensitive. Other values enable recording with a warning.

```yaml
env:                        # top-level: applied to all steps
  ANTHROPIC_API_KEY: sk-...
  TARGET_ENV: production

steps:
  deploy:
    command: ./deploy.sh
    env:                    # step-level: merged over top-level env
      TARGET_ENV: staging   # overrides top-level value for this step only
      LOG_LEVEL: debug
```

<a id="application-configuration"></a>
## Application configuration

Cruise stores application settings separately from workflow YAML in `$XDG_CONFIG_HOME/cruise/config.json` or `config.yaml`. If both exist, JSON wins. See [workflow config file resolution](#config-file-resolution) above for workflow selection and other settings.

<a id="ui-language"></a>
### UI language

Set `uiLanguage` in application configuration, or use `cruise config --set-language ja`. The alias `ui_language` is accepted on input. Supported explicit tags are `en`, `zh-CN`, `es`, `fr`, `pt-BR`, `ja`, and `ko`, with case and `_`/`-` variations accepted. `auto` means unspecified; `cruise config --set-language auto` clears the saved setting. Invalid values are rejected when setting or validating application configuration.

For CLI and TUI, `CRUISE_UI_LANGUAGE` takes precedence over `uiLanguage`, then system locale, then English. A blank or `auto` environment value defers to the next layer. Invalid environment values produce an English warning and are ignored. System locale detection uses the first nonempty `LC_ALL`, `LC_MESSAGES`, `LANG`, or `LANGUAGE`, without trying later variables when that value is unsupported. Windows also consults the user locale if environment variables do not determine one. macOS and Linux use environment variables only, so a GUI launch without them defaults to English.

The WebUI uses its browser's `cruise_ui_language` cookie when valid, otherwise the server process default. Selecting a language in the WebUI saves the cookie and reloads the page, without changing application configuration. UI language is independent of workflow `languages.pr` and `languages.plan`, which control generated prompts.

<a id="config-hot-reload"></a>
## Config hot-reload

During `cruise run`, the config file is checked for changes between steps. If the file has been modified (detected via mtime), the updated config is reloaded automatically without a restart. This lets you adjust prompts, add steps, or change settings while a session is running.

> **Note:** Hot-reload applies only to sessions started from an external config file, not an embedded built-in workflow. The current step must still exist in the new config for reload to take effect. Newly declared flags absent from the session's saved values are off.

<a id="rate-limit-retry"></a>
## Rate Limit Retry

Without an SDK fallback policy, cruise retries the same model with exponential
backoff when command stderr contains rate-limit wording or a standalone `429`
token (not incidental `429` substrings). SDK backends also treat `usage limit`,
`session limit`, and `overloaded` as limits:

- Initial delay: 2 seconds
- Maximum delay: 60 seconds
- Default retry count: 5 (override with `--rate-limit-retries`)

The SDK backends additionally support a fallback policy through an optional
`retry:` block or a workflow-level model array with fallback entries. An
explicit `retry:` block can configure the policy:

```yaml
retry:
  base_delay_ms: 500        # default: 500
  max_delay_ms: 300000      # default: 300000
  model_fallback: true      # default: true
  fallback_chains:
    default:
      - provider-a/model-a
      - provider-b/model-b
    "provider-a/*":
      - provider-c/*
```

With an SDK retry policy, a 429, 5xx, other 4xx, or network failure switches to the next fallback immediately; the same model is retried only when no usable fallback remains. Declaring `retry:` enables this classification and backoff even with `model_fallback: false` or empty chains. To keep historical behavior, omit `retry:` and use scalar or unset model fields. Workflow-level model arrays with fallback entries enable model fallback. The `command:` backend retains historical behavior and ignores `retry:`.
