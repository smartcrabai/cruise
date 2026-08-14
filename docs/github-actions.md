# cruise GitHub Action

Mention `@cruise` (configurable) on a GitHub Issue to have cruise plan, implement, and open a draft PR inside GitHub Actions -- similar in spirit to [`anthropics/claude-code-action`](https://github.com/anthropics/claude-code-action), but driving cruise's own plan -> implement -> PR workflow, always through the `sdk: pi` backend (`pi_agent_rust`, in-process -- no `claude` CLI install).

This action has **no pull-request mode**: it only reacts to `issues` (opened) and `issue_comment` (created) events. A comment made on a pull request is always ignored (PRs are "issues" at the GitHub API level, but this action explicitly excludes them).

## Quickstart

1. Install the [`cruise-agent` GitHub App](https://github.com/apps/cruise-agent/installations/new) on your repository (optional -- skip it to fall back to the workflow's `GITHUB_TOKEN`; see [How authentication works](#how-authentication-works)).
2. Add an `ANTHROPIC_API_KEY` and/or `OPENAI_API_KEY` repository secret (Settings -> Secrets and variables -> Actions). pi needs at least one.
3. Copy [`examples/cruise.yml`](../examples/cruise.yml) to `.github/workflows/cruise.yml`.

Then open an issue, or comment on one, with `@cruise plan <what you want>`. See [Typical workflow](#typical-workflow) for the full plan -> fix -> run -> review loop, [Commands](#commands) below for every mention form, [Providers](#providers) for using something other than Anthropic/OpenAI, and [Setup](#setup) for the detailed walkthrough (full workflow YAML, `permissions:`, version pinning).

## Commands

The first word after the `@cruise` mention (with or without a leading `/`, case-insensitive, trailing punctuation like `.`/`,`/`:` stripped before matching) selects what happens. Anything else -- including no word at all -- is treated as `run`.

| Mention | Command | What happens |
|---|---|---|
| `@cruise`, `@cruise run <request>`, `@cruise /run` | **run** | Resolve the plan (see below; any text typed after `run` is appended as extra instructions), create a session from it verbatim (no LLM planning call), execute it in a worktree, push a branch, and open a **draft** pull request. |
| `@cruise exec <request>`, `@cruise /exec` | **exec** | Resolve the plan (same as `run`, extra instructions included), then run it directly on the already-checked-out default branch and **push straight to that branch** (no PR). Advanced/opt-in -- see [exec caveats](#exec-caveats). |
| `@cruise plan <request>`, `@cruise /plan` | **plan** | Run an LLM planning call (`cruise plan`) on the issue's title + body and post the result as a new **plan-tracking comment**. Nothing is executed. (The text typed after `plan` in the triggering comment itself is currently not included -- only the issue's title/body feed the plan.) |
| `@cruise fix <feedback>`, `@cruise /fix <feedback>` | **fix** | Revise the most recent *trusted* plan-tracking comment using `<feedback>`, then **edit that same comment in place** with the revised plan. Fails with a clear message if there is no existing plan comment. |

**Plan resolution** (used by `run` and `exec`): the action looks at every comment on the issue for the last one that both contains the `<!-- cruise:plan -->` marker **and** was posted by this action itself (`cruise-agent[bot]` or `github-actions[bot]`, whichever token this run authenticated with -- see [how authentication works](#how-authentication-works)); comments from anyone else, even if they happen to contain the marker text, are never trusted as a plan source. If a trusted plan comment exists, its plan content is used, otherwise the issue's own title + body is used. Either way, any text typed after the command word in the comment/issue that triggered this run is appended as a "## Additional instructions from the triggering comment" section.

### Command grammar

Command parsing is intentionally strict and mechanical, not natural-language understanding:

- Only the **first whitespace-delimited word** right after the mention is checked against `run`/`exec`/`plan`/`fix` (optionally prefixed with `/`, case-insensitive, trailing `.,!?;:` stripped). Everything else in the message -- including further sentences -- has no bearing on which command runs.
- If the body has **multiple `@cruise` mentions** (e.g. a quoted reply that includes an earlier message), the **last** one is used, so replying to an old mention doesn't resurrect its command.
- Because matching is purely lexical, a plain-English sentence that happens to start with a command word after the mention is parsed as that command -- e.g. `@cruise fix the flaky test` is parsed as the **`fix` command** with feedback `the flaky test`, not as a free-form request to `run`. If there is no existing plan comment yet, this fails with a message telling you to run `@cruise plan` first, rather than silently doing something else.
- To avoid this kind of ambiguity, prefer the explicit slash form (`@cruise /run <request>`, `@cruise /exec <request>`) for free-form requests, and reserve the bare word form (`@cruise plan ...` / `@cruise fix ...`) for when you actually mean the `plan`/`fix` commands.

## Typical workflow

An end-to-end example, planning first:

1. **Open an issue** describing the task. Leave the trigger phrase (`@cruise`) out of the title and body -- see the warning below for why that matters.

   > **Add retry logic to the uploader**
   >
   > The S3 upload occasionally fails on flaky networks. It should retry with exponential backoff instead of failing the whole job.

2. **Ask for a plan**: comment `@cruise plan`. The action runs an LLM planning call and posts a new plan-tracking comment; nothing is executed yet.

3. **Iterate on the plan** as many times as needed: comment `@cruise fix also add a changelog entry and cover the timeout case`. The action edits the *same* tracking comment in place with the revised plan.

4. **Execute it**: comment `@cruise run`. The action creates a session from the (possibly revised) plan, implements it in a worktree, pushes a branch, and opens a pull request.

5. **Review and ready it**: cruise's pull requests are always opened as **drafts**, regardless of which command created them. Open the PR, review the diff, then mark it "Ready for review" (or `gh pr ready <number>`) before merging like any other PR.

> [!WARNING]
> If the issue's own title or body already contains the trigger phrase, step 2 never gets a chance to happen on its own -- the `issues: [opened]` trigger fires immediately on creation and defaults to `run`, jumping straight to implementation and a PR (see [command grammar](#command-grammar)). Keep the trigger phrase out of the issue itself when you want to review a plan first; only type it in a follow-up comment.

That immediate-run behavior is also a shortcut when you *don't* need a review step: `@cruise run <request>` (or just `@cruise <request>`, or an issue whose body already contains `@cruise`) skips the planning call entirely -- the issue's title + body becomes the plan directly, with `<request>` (if any) appended as additional instructions.

Multiple mentions on the same issue queue rather than race each other: a `plan` comment followed immediately by `fix` and then `run` still runs one at a time, in order (see the `concurrency:` block in [examples/cruise.yml](../examples/cruise.yml)) -- it's safe to keep commenting without waiting for each run to finish first.

## Setup

1. **Install the `cruise-agent` GitHub App** on your repository: [github.com/apps/cruise-agent/installations/new](https://github.com/apps/cruise-agent/installations/new). This is what lets the action authenticate as a scoped bot identity (`cruise-agent[bot]`) instead of the workflow's own `GITHUB_TOKEN` -- see [How authentication works](#how-authentication-works) below. You can skip this step; the action still runs, but falls back to `GITHUB_TOKEN` with the limitations described there.
2. Add an `ANTHROPIC_API_KEY` and/or `OPENAI_API_KEY` secret to your repository (Settings -> Secrets and variables -> Actions). At least one is required.
3. Copy [`examples/cruise.yml`](../examples/cruise.yml) to `.github/workflows/cruise.yml` (the version below is a condensed excerpt -- the file in `examples/` has a one-line comment above each section explaining what it's for):

   ```yaml
   name: Cruise

   on:
     issue_comment:
       types: [created]
     issues:
       types: [opened]

   jobs:
     cruise:
       if: |
         (github.event_name == 'issue_comment' && !github.event.issue.pull_request && contains(github.event.comment.body, '@cruise')) ||
         (github.event_name == 'issues' && (contains(github.event.issue.title, '@cruise') || contains(github.event.issue.body, '@cruise')))
       runs-on: ubuntu-latest
       # write-tests -> implement each run a full verification pass; 30
       # minutes (a common default) is too tight for larger repositories --
       # see the Troubleshooting FAQ below.
       timeout-minutes: 60
       permissions:
         contents: write
         pull-requests: write
         issues: write
         id-token: write # needed for the cruise-agent App token exchange; optional
       steps:
         - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7
           with:
             fetch-depth: 0
         - uses: smartcrabai/cruise@v1
           with:
             anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
   ```

4. Open an issue mentioning `@cruise`, or comment `@cruise <command> ...` on an existing one.

The workflow-level `if:` is only a coarse pre-filter (so unrelated events don't spin up a runner); the action independently re-checks the trigger phrase with a strict word-boundary match, verifies the commenter's permissions, and rejects PR comments before doing anything, so it is safe even if the pre-filter is removed.

**Minimum cruise version: v0.1.68** (the first release with `sdk: pi` support, which this action always uses). The default `cruise_version: latest` already satisfies this; pin an explicit tag if you want reproducible installs.

## Providers

cruise always executes through `sdk: pi` in this action (see [below](#sdk-pi----how-execution-works)). pi ships built-in definitions for several dozen providers -- each one just needs its API key, no `pi_models_json` required. Dedicated inputs exist for Anthropic/OpenAI keys; other built-in providers use [`env`](#env-input). For an endpoint pi doesn't already know about, use the `providers` input (plus `provider_api_keys` for its credentials) to have the action generate pi's `models.json` for you:

```yaml
with:
  providers: |
    {
      "openai-local": {
        "api": "openai-completions",
        "base_url": "https://gateway.example/v1",
        "models": ["gpt-local"]
      },
      "anthropic-local": {
        "api": "anthropic-messages",
        "base_url": "https://gateway.example/anthropic/v1/messages",
        "models": [
          {
            "id": "claude-local",
            "context_window": 200000,
            "cost": { "input": 3.0, "output": 15.0, "cache_read": 0.3, "cache_write": 3.75 }
          }
        ],
        "compat": { "max_tokens_field": "max_completion_tokens" }
      }
    }
  provider_api_keys: |
    openai-local=${{ secrets.OPENAI_LOCAL_KEY }}
    anthropic-local=${{ secrets.ANTHROPIC_LOCAL_KEY }}
  model: openai-local/gpt-local
  plan_model: anthropic-local/claude-local
```

### Provider schema

Each key of `providers` is a provider id (letters/digits/`.`/`_`/`-`, must start with a letter or digit) mapping to an object with:

- **`api`** (required) -- one of the 7 values in the [API table](#supported-api-values) below.
- **`base_url`** (required) -- a non-empty `http://`/`https://` URL with no whitespace.
- **`models`** (required, non-empty, unique) -- each entry is either a bare model id string, or an object:
  - `id` (required) -- the model id.
  - `name`, `api` (per-model override, same 7-value allowlist), `reasoning` (boolean) -- all optional.
  - `input` (optional) -- an array containing only `"text"`/`"image"` (pi's `ModelConfig.input` silently drops any other value, so a typo would otherwise go unnoticed instead of erroring).
  - `cost` (optional) -- if present, **all four** of `input`, `output`, `cache_read`, `cache_write` are required (pi's `ModelCost` has no defaults, so a partial `cost` object fails to parse the whole `models.json`). All four are numbers, **priced per million tokens**.
  - `context_window`, `max_tokens` (optional) -- positive integers.
  - `headers`, `compat` (optional) -- same shape as the provider-level fields below, merged on top (model wins per-key/per-field).
- **`headers`** (optional) -- a non-empty object of header name -> value, applied to every model of this provider (per-model `headers` merge on top, model wins per-key).
- **`auth_header`** (optional, boolean) -- forwarded to pi verbatim; only affects pi's own CLI-startup readiness check, not the underlying request adapter (see `no_auth` below for the mechanism that actually skips a request-time credential check).
- **`compat`** (optional) -- a non-empty object; recognized keys: `supports_store`, `supports_developer_role`, `supports_reasoning_effort`, `supports_usage_in_streaming`, `supports_tools`, `supports_streaming`, `supports_parallel_tool_calls`, `force_adaptive_thinking` (booleans); `max_tokens_field`, `system_role_name`, `stop_reason_field`, `thinking_format` (non-empty strings); `custom_headers`, `thinking_level_map` (non-empty objects); `open_router_routing`, `vercel_gateway_routing` (opaque JSON objects pi merges verbatim into the request body -- not otherwise validated).
- **`no_auth`** (optional, boolean) -- see [Keyless endpoints](#keyless-endpoints-no_auth) below.

A header/`apiKey` value may be `env:VAR_NAME` or `file:/path/to/secret` (resolved by pi at request time); a value starting with `!` is rejected outright -- pi would otherwise run it as a shell command (`sh -c`), and a generated config must never become a shell-exec vector.

Any key not on these lists -- at the provider, model, or `compat` level -- is rejected with an error naming the offending key. pi's own structs never use `#[serde(deny_unknown_fields)]`, so a misspelled field (e.g. `authHeader` instead of `auth_header`) would otherwise be silently ignored by pi rather than doing what you intended; rejecting it here is the main reason to prefer `providers` over hand-writing `pi_models_json`.

### Supported API values

| `api` | needs beyond `base_url` + one key |
|---|---|
| `anthropic-messages` | nothing extra -- sent as `X-API-Key` |
| `openai-completions` | nothing extra -- sent as `Authorization: Bearer` |
| `openai-responses` | nothing extra -- sent as `Authorization: Bearer` |
| `google-generative-ai` | nothing extra -- sent as `x-goog-api-key` |
| `azure-openai-responses` | shape the deployment name and API version into `base_url` (e.g. `.../deployments/<name>?api-version=2024-12-01-preview`), or set the model's `id` to the deployment name -- sent as `api-key` |
| `bedrock-converse-stream` | the key must be an AWS Bedrock **bearer** API key, and the runner must have **no** ambient `AWS_*` credential env vars (they take priority and need a second secret plus SigV4 signing, not "one key"); region comes from `base_url`'s host |
| `cohere-chat` | nothing extra -- sent as `Authorization: Bearer` (routes to the same adapter as pi's built-in `cohere` provider) |

Three more `api` values exist in pi but are **not** accepted here, because a static per-provider key genuinely cannot satisfy them: `openai-codex-responses` (needs a ChatGPT OAuth JWT from `/login`), `google-gemini-cli` and `google-vertex` (both need a GCP OAuth token pi doesn't mint itself). Use [`pi_models_json`](#combining-providers-with-pi_models_json) for these instead.

### Keyless endpoints (`no_auth`)

Set `"no_auth": true` on a provider entry to point at an endpoint that takes no credential at all (a local proxy, a dev gateway, etc.):

- That provider must **not** appear in `provider_api_keys` (listing one there is a contradiction and hard-fails); `provider_api_keys` may be left empty entirely once every `providers` entry is `no_auth`.
- No `apiKey` is emitted for it. Instead the action injects a non-empty placeholder `authorization` header, which every supported `api` value accepts as a credential override -- **unless** the entry already supplies its own non-blank auth-override header, in which case nothing is injected and your value is used as-is. Which header names count is **per adapter**, because each one only inspects its own: `authorization` works for all seven, and additionally `x-api-key` for `anthropic-messages`, `x-goog-api-key` for `google-generative-ai`, `api-key` for `azure-openai-responses`. A name that belongs to a *different* adapter (say `api-key` on a `cohere-chat` entry) does not count, and the placeholder is injected anyway -- pi would otherwise reject the request with `Missing API key for provider`. The match is case-insensitive, applies to both `headers` and `compat.custom_headers`, and a blank or whitespace-only value never counts (pi trims before testing). When a per-model `api` overrides the provider's, a header only counts if *every* adapter involved recognizes it.
- Rejected for `api: bedrock-converse-stream` -- AWS auth is never truly absent, so `no_auth` there hard-fails rather than emitting a config that would fail at request time. This applies to a per-model `api` override too, not just the provider-level value.
- Rejected outright when the provider id collides with one of pi's built-in ids (see [Provider-id collisions](#provider-id-collisions-with-pis-built-ins)). A collision makes pi ignore your `api` field, so the action cannot tell which adapter will run, and therefore cannot know which header would suppress its credential check. Rename the provider instead.

### Combining `providers` with `pi_models_json`

`providers` and `pi_models_json` **compose** rather than conflict: the action generates a `models.json` from `providers` first, then deep-merges `pi_models_json` on top of it (`pi_models_json` wins on any key both define -- it's the raw escape hatch). Arrays -- notably a provider's `models` list -- are **replaced wholesale, not concatenated**, so a `pi_models_json` overlay that sets `providers.some-id.models` fully replaces that id's model list rather than appending to it.

`pi_models_json` remains necessary for things `providers` deliberately doesn't expose:

- The three excluded `api` values above.
- pi's built-in-model **override mode**: a `models.json` provider entry with the `models` key *absent* (not even `[]`) patches pi's built-in models for that provider id instead of replacing them. `providers` always emits `models`, so it can only ever express the replace behavior -- the absent-vs-`[]` distinction is subtle enough that it's left to the raw escape hatch on purpose.
- Any future pi `models.json` field this action hasn't mapped yet.

### Provider-id collisions with pi's built-ins

Avoid a `providers` id that collides (case-insensitively) with one of pi's own built-in provider ids or aliases (e.g. `anthropic`, `openai`, `azure`, `bedrock`, `cohere`, ...) -- pi's request dispatch matches the provider id *before* looking at the `api` string, so a collision silently breaks the entry two ways: pi resolves the credential from that built-in provider's own source first (your `provider_api_keys` value may never be used), **and** your `api` field is ignored entirely in favor of whatever adapter that built-in id normally routes to. The action can't stop you from picking a colliding id (overriding a built-in is a legitimate advanced move), but it emits a `::warning::` naming both effects when it detects one. The one case it does reject outright is a colliding id combined with `no_auth: true`, where the ignored `api` field makes the keyless mechanism unverifiable rather than merely surprising.

| Provider | pi provider id | Env var(s) | Example model reference |
|---|---|---|---|
| Anthropic | `anthropic` | `ANTHROPIC_API_KEY` (dedicated `anthropic_api_key` input) | `anthropic/claude-sonnet-4-6` |
| OpenAI | `openai` | `OPENAI_API_KEY` (dedicated `openai_api_key` input) | `openai/gpt-5.2` |
| Kimi for Coding | `kimi-for-coding` | `KIMI_API_KEY` (via `env`) | `kimi-for-coding/kimi-for-coding` -- see the [inline example](#kimi-for-coding-example) below |
| Google Gemini | `google` | `GOOGLE_API_KEY` or `GEMINI_API_KEY` (via `env`) | `google/gemini-3-pro-preview` |
| Groq | `groq` | `GROQ_API_KEY` (via `env`) | `groq/llama-3.3-70b-versatile` |
| Mistral AI | `mistral` | `MISTRAL_API_KEY` (via `env`) | `mistral/mistral-large-latest` |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY` (via `env`) | `deepseek/deepseek-chat` |
| xAI (Grok) | `xai` | `XAI_API_KEY` (via `env`) | `xai/grok-4` |
| Moonshot AI | `moonshotai` | `MOONSHOT_API_KEY` or `KIMI_API_KEY` (via `env`) | `moonshotai/kimi-k2-turbo-preview` |

Model IDs move fast -- treat the examples above as illustrative and check the provider's own docs if a reference stops resolving. Several more providers work the same way with no extra config at all (OpenRouter, Cerebras, Fireworks, Together AI, Perplexity, and others). Use the `providers`/`provider_api_keys` inputs above for an endpoint pi doesn't already know about, such as a self-hosted OpenAI-compatible gateway -- see [`examples/cruise-openai-compatible.yml`](../examples/cruise-openai-compatible.yml) for a full drop-in workflow. `pi_models_json` remains available as the raw escape hatch for schemas those two inputs can't express.

### Kimi for Coding example

[Kimi for Coding](https://api.kimi.com/coding/) (an Anthropic-compatible endpoint) is what this repository's own dogfood workflow uses (`.github/workflows/cruise.yml`). See [`examples/cruise-kimi.yml`](../examples/cruise-kimi.yml) for the full drop-in workflow; the cruise-specific part is just:

```yaml
- uses: smartcrabai/cruise@v1
  with:
    model: kimi-for-coding/kimi-for-coding
    plan_model: kimi-for-coding/kimi-for-coding
    env: |
      KIMI_API_KEY=${{ secrets.KIMI_API_KEY }}
```

(`kimi-for-coding` is a virtual model id the Kimi backend remaps to its latest coding model.)

## sdk: pi -- how execution works

This action always forces `CRUISE_SDK=pi` in the environment before invoking cruise, regardless of what any config file says (`command:`/`sdk:` in a repo's own `cruise.yaml` are overridden). This means:

- **No `claude` CLI is installed.** cruise drives `pi_agent_rust` directly, in-process.
- **Authentication** is resolved entirely by pi, in this order: an explicit key (not exposed here) > pi's stored `~/.pi/agent/auth.json` OAuth/Bearer credentials (only relevant on a persistent self-hosted runner where someone ran `pi login` ahead of time) > provider API-key env vars (`ANTHROPIC_API_KEY`/`OPENAI_API_KEY` from dedicated inputs, generated `CRUISE_PROVIDER_API_KEY_N` values from `provider_api_keys`, or any other provider's key passed through `env`). The gate step fails clearly when `anthropic_api_key`, `openai_api_key`, `provider_api_keys`, `providers`, `pi_models_json`, and `env` are all empty -- `providers`/`pi_models_json` count as credential sources too, since an all-`no_auth` `providers` config or a `pi_models_json` with its own literal `apiKey` values needs no `provider_api_keys` line at all.
- **Model selection** (`model`/`plan_model` inputs, mapped to `CRUISE_MODEL`/`CRUISE_PLAN_MODEL`) uses pi's model-reference format, not seher mode keys:
  - `"provider/model"`, optionally with `:thinking` (e.g. `openai-codex/gpt-5.5:xhigh`) -- selects that provider and model explicitly.
  - `"model"` (no `/`) -- pi searches its own model registry for that id.
  - Empty (default) -- pi auto-selects a provider/model from its built-in preference order, picking the first one with usable credentials.
- **Custom endpoints / providers**: the concise `providers`/`provider_api_keys` inputs generate a pi `models.json` for you -- see [Providers](#providers) above, including a worked example ([`examples/cruise-openai-compatible.yml`](../examples/cruise-openai-compatible.yml)). `pi_models_json` remains the raw escape hatch for whatever `providers` [doesn't cover](#combining-providers-with-pi_models_json): paste the raw contents of a pi `models.json` file directly, and it deep-merges on top of anything `providers` generated. The action writes the result to `$RUNNER_TEMP/pi-agent/models.json` and points `PI_CODING_AGENT_DIR` at that directory for the run.

### Zero-config default: pi auto-selects the model

The default (no `model`/`plan_model` input) is pi's own auto-selection -- no model configuration is required to get started. This matters because cruise's *built-in* default workflow (used when no config file exists at all) hardcodes `model: sonnet` / `plan_model: opus` as literal strings, which under `sdk: pi` would be interpreted as bare pi model-registry ids rather than seher mode keys -- and pi has no id named exactly `sonnet`/`opus` (real ids look like `claude-sonnet-4-6`), so relying on that raw built-in default would fail to resolve a model. To avoid this, when `config` is empty **and** the repository has no config of its own, this action generates a default config itself (see [config resolution](#config-resolution) below) that mirrors cruise's `write-tests -> implement` workflow but deliberately omits `model`/`plan_model`, letting pi auto-select based on whichever of `anthropic_api_key`/`openai_api_key` is set. Set the `model`/`plan_model` inputs explicitly (in pi's reference format) if you want a specific model instead.

## config resolution

- **`config` input set** -- resolved to an absolute path and exported as `CRUISE_CONFIG`. Used by the `run`/`plan`/`fix` commands.
- **`config` input empty, and the repository already has its own config** (`cruise.yaml`/`cruise.yml`/`.cruise.yaml`/`.cruise.yml` at the checkout root, or any YAML file under `.cruise/`) -- `CRUISE_CONFIG` is left unset entirely and cruise's own resolver picks that file up. See [`examples/repo-cruise.yaml`](../examples/repo-cruise.yaml) for a config you can commit as your own `cruise.yaml`.
- **`config` input empty, and the repository has no config of its own** -- this action generates a default config (`sdk: pi`, `write-tests -> implement` steps with prompts embedded verbatim from this action's `prompts/write-test-first.md`/`prompts/implement-after-tests.md`, no `model`/`plan_model`) and exports it as `CRUISE_CONFIG`. See [above](#zero-config-default-pi-auto-selects-the-model) for why `model`/`plan_model` are omitted.
- **`exec` always uses its own generated config**, regardless of `config` or the two cases above: a minimal `sdk: pi` config with a single `implement` step whose prompt is `"{input}"` (also without `model`/`plan_model`). `cruise exec` binds the whole plan text to `{input}` and never runs a planning step (`plan.md` stays empty), so a `{plan}`-based config would silently receive an empty prompt.

In every case, the `model`/`plan_model` inputs (`CRUISE_MODEL`/`CRUISE_PLAN_MODEL` env overrides, applied by cruise itself) take priority over whatever a config file does or doesn't set.

Avoid `option:` steps in a config used for CI -- they prompt interactively and there is no terminal attached in Actions.

## `env` input

Pass extra environment variables into the cruise process with the `env` input, one `KEY=VALUE` per line:

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    env: |
      MY_TOOL_TOKEN=${{ secrets.MY_TOOL_TOKEN }}
      FEATURE_FLAG=true
```

Blank lines and lines starting with `#` are ignored. Each value is masked (`::add-mask::`) before being exported. Reserved names -- `GITHUB_TOKEN`, `GH_TOKEN`, `PI_CODING_AGENT_DIR`, `PATH`, `HOME`, `SHELL`, the git identity vars (`GIT_AUTHOR_*`/`GIT_COMMITTER_*`), the `XDG_*` vars, and anything prefixed `CRUISE_`/`GITHUB_`/`ACTIONS_`/`RUNNER_` -- are skipped with a `::warning::` instead of being overridden, since the action itself manages them. (`CRUISE_*` is a prefix rule, not just `CRUISE_SDK`/`CRUISE_CONFIG`: reach for the dedicated `model`/`plan_model`/`config` inputs rather than a raw `CRUISE_*` variable.) `ANTHROPIC_API_KEY`/`OPENAI_API_KEY` are **not** reserved here -- passing them via `env` (instead of, or alongside, the dedicated `anthropic_api_key`/`openai_api_key` inputs) is a supported way to satisfy the credential gate.

## exec caveats

`exec` pushes **directly to the default branch**, with no PR and no review step. This:

- **Interacts badly with branch protection.** If the default branch requires PRs, status checks, or reviews before merging, cruise's direct push will simply fail (surfaced as a failed run). `exec` is meant for repositories that intentionally allow direct pushes to their default branch, or bypass rules for the actor/token cruise uses.
- **Is advanced/opt-in.** There is no undo beyond `git revert`. Prefer `run` (which opens a draft PR you can review) unless you specifically want unattended direct pushes.
- Skips the commit+push step entirely (and reports success) if `cruise exec` produced no file changes.

## How authentication works

By default (`github_token` input left empty), the action tries to authenticate as the `cruise-agent` GitHub App instead of using the workflow's own `GITHUB_TOKEN`:

1. With `permissions: id-token: write` granted, GitHub Actions gives the job a short-lived OIDC token identifying the workflow, repository, and run.
2. The `token` step exchanges that OIDC token for a **repository-scoped, short-lived cruise-agent App installation token** by calling the `token_exchange_url` service (`POST` with `Authorization: Bearer <OIDC token>`, no body). The exchange service verifies the OIDC token's `repository` claim server-side and only ever issues a token scoped to that repository's installation -- it cannot mint a token for a repository the calling workflow doesn't belong to.
3. cruise runs and pushes commits authenticated as `cruise-agent[bot]` using that token. Commits created by the action also add the user who triggered the mention as a `Co-authored-by` trailer using that user's GitHub-provided noreply address.
4. After the run finishes (success or failure), the action revokes the token (`DELETE /installation/token`) so it can't be reused past the job's lifetime.

If the App isn't installed on the repository (the exchange returns 404), the OIDC token can't be obtained (e.g. `id-token: write` wasn't granted), or the exchange service is unreachable, the action **falls back to the workflow's `GITHUB_TOKEN`** and logs why (a `::notice::` with the App install link for the "not installed" case, a `::warning::` otherwise). cruise still runs in that case, but with two `GITHUB_TOKEN`-specific limitations: draft PRs it opens won't trigger other `on: pull_request` workflows (see [Security](#security)), and commits use `github-actions[bot]` as the author/committer instead of `cruise-agent[bot]`.

## Bring your own token

Set `github_token` explicitly (e.g. to a PAT, or a token from your own GitHub App) to skip the OIDC exchange entirely -- the action uses that token for every GitHub API call and push, and never attempts the exchange or revocation:

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    github_token: ${{ secrets.MY_PAT }}
```

## Self-hosting the token exchange

The token-exchange service is a small Cloudflare Worker that verifies GitHub Actions OIDC tokens and mints GitHub App installation tokens; see [`../token-exchange/README.md`](../token-exchange/README.md) for its source and deployment instructions. If you run your own instance (your own GitHub App, your own Worker), point the action at it with `token_exchange_url`:

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    token_exchange_url: https://your-worker.your-subdomain.workers.dev/token
```

Set `token_exchange_url: ""` to disable the exchange outright and always use `github_token`/`GITHUB_TOKEN`.

In both cases the action posts a tracking comment when it starts and rewrites it with the outcome when it finishes -- success with links (a PR for `run`, a commit for `exec`, the plan comment for `plan`/`fix`), or failure with a link to the run. Run logs are deliberately **never** copied into the comment (see [Security](#security)).

## Inputs

| Input | Default | Description |
|---|---|---|
| `anthropic_api_key` | *(empty)* | Anthropic API key for pi. At least one of `anthropic_api_key`, `openai_api_key`, `provider_api_keys`, `providers`, `pi_models_json`, or `env` must be non-empty. |
| `openai_api_key` | *(empty)* | OpenAI API key for pi. At least one of `anthropic_api_key`, `openai_api_key`, `provider_api_keys`, `providers`, `pi_models_json`, or `env` must be non-empty. |
| `github_token` | *(empty)* | Token for GitHub API calls (permission checks, comments, PRs, pushes). Empty (default) tries the cruise-agent App OIDC token exchange first, falling back to the workflow's `GITHUB_TOKEN`; set explicitly to skip the exchange and use that token instead. See [How authentication works](#how-authentication-works). |
| `token_exchange_url` | *(cruise-agent's hosted exchange)* | URL of the token-exchange service. Empty disables the exchange (always falls back to `github_token`/`GITHUB_TOKEN`). See [Self-hosting the token exchange](#self-hosting-the-token-exchange). |
| `trigger_phrase` | `@cruise` | Phrase that must appear (word-boundary match) in the body to trigger a run. |
| `cruise_version` | `latest` | cruise release to install (`latest` or a tag like `v0.1.68`). Requires v0.1.68+. |
| `config` | *(empty)* | Path to a cruise workflow config YAML in your repo, used by `run`/`plan`/`fix` (sets `CRUISE_CONFIG`). Empty lets cruise's own resolver pick a config from the checkout, or its built-in default. No effect on `exec`. |
| `model` | *(empty)* | Overrides `CRUISE_MODEL`, in pi's model-reference format (`provider/model[:thinking]`, a bare model id, or empty for auto-select). |
| `plan_model` | *(empty)* | Overrides `CRUISE_PLAN_MODEL` (the `plan`/`fix` commands' planning step), same format as `model`. |
| `pi_models_json` | *(empty)* | Raw contents of a pi `models.json` file. When `providers` is also set, deep-merged on top of the document generated from it (`pi_models_json` wins on shared keys). The result is written to `$RUNNER_TEMP/pi-agent/models.json` with `PI_CODING_AGENT_DIR` pointed at it. See [Providers](#providers). |
| `providers` | *(empty)* | JSON provider map. Each value requires `api` (one of 7 supported values), `base_url`, and a non-empty `models` array (string or object entries); optional `headers`, `auth_header`, `compat`, `no_auth`. See [Providers](#providers) for the full schema. |
| `provider_api_keys` | *(empty)* | One `provider-id=API key` per line for `providers`; blank/comment lines ignored and the first `=` separates the key. Required for every `providers` entry that isn't `no_auth: true`; may be left empty when every entry is `no_auth`. |
| `env` | *(empty)* | Extra `KEY=VALUE` lines exported (masked) into the cruise process. Reserved names are skipped with a warning. |
| `allowed_bots` | *(empty)* | Comma-separated bot logins (without `[bot]`) allowed to trigger cruise, or `*` for any bot. Empty blocks all bots. |
| `git_user_name` | *(empty)* | git `user.name` for commits this action/cruise creates. Empty resolves to `cruise-agent[bot]` when the run used the App token, otherwise `github-actions[bot]`. |
| `git_user_email` | *(empty)* | git `user.email` for those commits. Empty resolves to match `git_user_name`'s default. Commits still add the triggering user as a `Co-authored-by` trailer when GitHub's event payload includes their login and numeric user id. |

## Outputs

| Output | Description |
|---|---|
| `command` | `"run"`, `"exec"`, `"plan"`, or `"fix"` -- the command parsed from the mention (empty if the gate skipped the run). |
| `session_id` | The cruise session ID that was created. |
| `pr_url` | URL of the pull request cruise opened (the `run` command only). |
| `commit_url` | URL of the commit cruise pushed to the default branch (the `exec` command only). |
| `plan_comment_url` | URL of the plan-tracking comment cruise posted or edited (the `plan`/`fix` commands only). |
| `conclusion` | `success`, `failure`, or `skipped` (mention didn't match, or actor wasn't authorized). |
| `used_app` | `"true"` if the run authenticated with a cruise-agent App installation token, `"false"` if it used `github_token`/`GITHUB_TOKEN`, or an empty string if the gate step skipped the run before the `token` step ran (mention didn't match / actor not authorized). |

## Testing the action's step scripts

The composite action's step scripts (`action/scripts/*.sh`) are exercised directly by the suites in `scripts/test_action_*.sh`. The suites fake the runner contract (`GITHUB_ENV`, `GITHUB_OUTPUT`, `RUNNER_TEMP`) and use PATH stubs for `gh`, `curl`, and `cruise`, so the tests are fully hermetic: everything is written under a temp directory, no network access is required, and they run on bash 3.2 as well as bash 5.

| Test suite | Step scripts covered |
|---|---|
| `scripts/test_action_gate.sh` | `action/scripts/gate.sh` |
| `scripts/test_action_token.sh` | `action/scripts/app-token.sh` and `action/scripts/revoke-token.sh` |
| `scripts/test_action_config_install.sh` | `action/scripts/resolve-config.sh` and `action/scripts/install.sh` |
| `scripts/test_action_comments.sh` | `action/scripts/comment-start.sh` and `action/scripts/finalize.sh` |
| `scripts/test_action_run.sh` | `action/scripts/run.sh` and `action/scripts/lib/plan.sh` |
| `scripts/test_action_provider_config.sh` | `action/scripts/setup-env.sh` and the credential gate |

Run a single suite locally with:

```bash
bash scripts/test_action_<name>.sh
```

To run every suite the way CI does, loop over `scripts/test_action_*.sh`:

```bash
status=0
for suite in scripts/test_action_*.sh; do
  echo "== $suite"
  bash "$suite" || status=1
done
exit "$status"
```

The shared harness is `scripts/lib/action_test_harness.sh`. Any new suite named `scripts/test_action_*.sh` is picked up automatically by the `Test GitHub Action step scripts` step in `.github/workflows/ci.yml`.

## Security

- **Only repository collaborators can trigger cruise.** The action calls the GitHub collaborator-permission API for the commenting/mentioning user and requires write access (the API reports the `maintain` role as `write`, so maintainers qualify; `triage` and `read` do not). Bot actors are rejected unless explicitly added to `allowed_bots`.
- **No PR mode.** `issue_comment` events on a pull request (`.issue.pull_request` present) are always denied, regardless of the trigger phrase.
- **Plan comments are trust-checked, not just marker-checked.** `run`/`exec`/`fix` only treat a comment as an authoritative plan source if it was posted by `cruise-agent[bot]`/`github-actions[bot]` *and* contains the plan marker. Without this, a commenter without write access could post a fake `<!-- cruise:plan -->` comment that a maintainer's later `@cruise run` would execute unreviewed (only the *mention itself* requires an authorized actor -- any other comment on a public issue does not).
- **The token exchange issues repository-scoped tokens only.** The exchange service validates the `repository` claim embedded in the caller's GitHub Actions OIDC token server-side before minting an installation token, so a workflow can only ever obtain a token scoped to the repository it is actually running in -- never another repository the App happens to be installed on. `permissions: id-token: write` only lets the job *request* that OIDC token from GitHub; it grants no GitHub API access by itself.
- **Unattended execution.** cruise drives pi (and, transitively, whatever tools your workflow config allows) without per-action confirmation prompts -- required for unattended CI use, but it also means a successful prompt-injection (see below) has the same blast radius as the workflow's own GitHub token and runner. Only grant the workflow the `permissions:` it needs (`contents: write`, `pull-requests: write`, `issues: write`, plus `id-token: write` if you want the App token exchange), and treat the provider API keys, `GITHUB_TOKEN`, and any App token this action obtains as you would any other CI secret with write access to your repository.
- **Prompt injection.** Issue bodies and fix feedback are attacker-controlled text that gets embedded in the prompt sent to the model. The action strips hidden-instruction vectors -- HTML comments, `<img>` tags (alt-text payloads), and zero-width/bidi-control Unicode characters -- from that raw GitHub-sourced text before it becomes planning input. This is a mitigation, not a guarantee: instructions written as plain visible text cannot be filtered out. Don't grant this action to a workflow with secrets or permissions beyond what a successful "the agent did whatever the issue text said" outcome would be acceptable for.
- **Run logs are never posted back to the issue.** The failure comment links to the Actions run instead of quoting log output. Agent output can contain anything the model was coaxed into printing (including environment values), and GitHub's secret masking only applies to the Actions log viewer -- not to text re-posted through the API -- so copying logs into a public comment would be an exfiltration channel. Logs stay on the run page, which follows your repository's access controls.
- **The installer is fetched over TLS but not checksum-verified.** The action installs cruise via its release installer script (`curl | sh`). This is the same trust model as rustup et al., but it does mean a compromise of the download endpoint would run attacker code with the job's secrets. Pin `cruise_version` to a tag if you want to at least avoid silently tracking `latest`.
- **PRs opened via `GITHUB_TOKEN` don't trigger other workflows.** This is a deliberate GitHub Actions anti-recursion rule, and it only applies to the plain `GITHUB_TOKEN` fallback path -- PRs/pushes made with the cruise-agent App's installation token (the default, when the App is installed) trigger `on: pull_request` workflows normally, since GitHub treats App-authenticated actions as coming from a distinct actor. If you're on the `GITHUB_TOKEN` fallback and need CI to run on cruise's PRs, either install the App, use your own PAT/GitHub App token via `github_token`, or add a step that closes/reopens the PR.
- **`exec` pushes directly to the default branch.** See [exec caveats](#exec-caveats).
- **Runner isolation.** Each run points `XDG_DATA_HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` at `$RUNNER_TEMP`, so cruise's session/worktree state never leaks between jobs and never touches a persistent runner's home directory.

## Troubleshooting

- **Nothing happens after mentioning `@cruise`.** Check the workflow run list for a skipped/no-op run: the gate step logs why it declined (event type, action, missing trigger phrase, PR comment, or insufficient actor permission).
- **"'anthropic_api_key', 'openai_api_key', 'provider_api_keys', 'providers', 'pi_models_json', and 'env' are all empty".** Set at least one dedicated key, `providers`/`provider_api_keys` for a generated provider, `pi_models_json`, or a provider key via `env` (e.g. `KIMI_API_KEY=...`).
- **"actor '&lt;login&gt;' has insufficient permission: '&lt;permission&gt;'".** The commenter needs `write`, `maintain`, or `admin` access to the repository.
- **"No existing plan comment found" (fix).** Run `@cruise plan` first; `fix` only edits an existing plan-tracking comment, it doesn't create one.
- **"cruise completed but no pull request was created" (run).** cruise ran (and may have pushed a branch), but `gh pr create` failed. Check that the workflow grants `permissions: pull-requests: write` and that branch protection / repository rules allow creating PRs from the pushed branch.
- **`exec`'s push fails.** Usually branch protection on the default branch -- see [exec caveats](#exec-caveats).
- **Model resolution errors.** With no `config` input and no repository config, this action already generates a `model`/`plan_model`-free default so pi auto-selects (see [zero-config default](#zero-config-default-pi-auto-selects-the-model)). If you *do* have your own `config` and see this, set the `model`/`plan_model` inputs explicitly in pi's reference format, or remove any leftover seher-style mode keys (e.g. `sonnet`/`opus`) from your config's `model:`/`plan_model:`.
- **Run always falls back to `GITHUB_TOKEN` (`used_app` output is `false`).** Check, in order: the `cruise-agent` App is installed on this repository ([install link](https://github.com/apps/cruise-agent/installations/new)); the workflow grants `permissions: id-token: write`; `token_exchange_url` is not empty and reachable. The `token` step's log line explains which of these failed.
- **Self-hosted runners** need `git`, `curl`, `jq`, `python3`, and the `gh` CLI on `PATH` (all preinstalled on GitHub-hosted runners).

### FAQ

- **Why did my brand-new issue immediately turn into a PR instead of posting a plan first?** Its title or body already contained the trigger phrase, so the `issues: [opened]` event fired the default `run` command right away -- see the warning in [Typical workflow](#typical-workflow). Leave the trigger phrase out of the issue itself and comment `@cruise plan` afterward if you want to review a plan first.
- **The pull request cruise opened won't merge / doesn't show up as ready.** All PRs from `run` (and, transitively, `exec`'s equivalent for direct pushes -- though that path has no PR at all) are opened as **drafts**, unconditionally. Mark it "Ready for review" yourself once you've reviewed the diff; see [Typical workflow](#typical-workflow).
- **My run timed out around 30 minutes.** `write-tests -> implement` (or your own config's equivalent steps) each run a full verification pass (formatting, linting, the whole test suite); on a large repository this measured over 30 minutes end to end during testing. `timeout-minutes: 60` (as used in [`examples/cruise.yml`](../examples/cruise.yml)) is the recommended starting point -- raise it further for slower test suites.
- **My second `@cruise` comment on the same issue seems stuck, not running.** It's queued, not lost: the `concurrency:` group in the example workflows serializes runs per-issue (no `cancel-in-progress`), so a `plan` -> `fix` -> `run` sequence executes one step at a time in order. Check the Actions run list -- the earlier run is probably still in progress.
- **cruise's PR didn't trigger my CI workflow.** Expected when running on the `GITHUB_TOKEN` fallback (no App installed) -- see [Security](#security) and [How authentication works](#how-authentication-works).
