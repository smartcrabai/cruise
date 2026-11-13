**Languages:** [English](../en/github-actions.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/github-actions.md) · [简体中文](../zh/github-actions.md) · [Index](README.md)

<a id="cruise-github-action"></a>
<a id="github-actions"></a>
# cruise GitHub Action

Mention `@cruise` (configurable) on a GitHub Issue to have cruise plan and implement inside GitHub Actions, and open a draft PR when the successful run leaves branch commits and automatic PR creation is enabled -- similar in spirit to [`anthropics/claude-code-action`](https://github.com/anthropics/claude-code-action), but driving cruise's own plan -> implement -> optional PR workflow through `sdk: jcode` unless the resolved workflow selects another backend -- the action installs the `jcode` CLI and provisions its credentials for you.

This action has **no pull-request mode**: it only reacts to `issues` (opened) and `issue_comment` (created) events. A comment made on a pull request is always ignored (PRs are "issues" at the GitHub API level, but this action explicitly excludes them).

> Note: restricted prompt steps (`permission: read-only | edit`) run unattended with denied tools rather than prompts, and a read-only step fails if it changes the workspace.

<a id="quickstart"></a>
## Quickstart

1. Install the [`cruise-agent` GitHub App](https://github.com/apps/cruise-agent/installations/new) on your repository (optional -- skip it to fall back to the workflow's `GITHUB_TOKEN`; see [How authentication works](#how-authentication-works)).
2. Add an `ANTHROPIC_API_KEY` and/or `OPENAI_API_KEY` repository secret (Settings -> Secrets and variables -> Actions). jcode needs at least one.
3. Copy [`examples/cruise.yml`](../../examples/cruise.yml) to `.github/workflows/cruise.yml`.

Then open an issue, or comment on one, with `@cruise plan <what you want>`. See [Typical workflow](#typical-workflow) for the full plan -> fix -> run -> review loop, [Commands](#commands) below for every mention form, [Providers](#providers) for using something other than Anthropic/OpenAI, and [Setup](#setup) for the detailed walkthrough (full workflow YAML, `permissions:`, version pinning).

<a id="commands"></a>
## Commands

The first word after the trigger phrase selects the command (case-insensitive, with an optional leading `/`). `plan`, `fix`, `exec`, and `run` select that command; `/tell` requires its slash. No word or an unknown word defaults to `run`.

Because matching is lexical, a sentence that starts with a command word is parsed as that command; for example, `@cruise fix the flaky test` selects `fix`. Use the slash form (`@cruise /run <request>` or `@cruise /exec <request>`) for free-form requests. If a comment has multiple `@cruise` mentions, only the last is used.

| Mention | Command | What happens |
|---|---|---|
| `@cruise`, `@cruise run <request>`, `@cruise /run` | **run** | Resolve the plan (see below; text after `run` is appended as extra instructions), create a session from it verbatim (without an LLM planning call), and execute it in a worktree. By default, a successful run that leaves branch commits opens a draft pull request. |
| `@cruise exec <request>`, `@cruise /exec` | **exec** | Resolve the plan (same as `run`, extra instructions included), then run it directly on the already-checked-out default branch and **push straight to that branch** (no PR). Advanced/opt-in -- see [exec caveats](#exec-caveats). |
| `@cruise plan <request>`, `@cruise /plan` | **plan** | Run an LLM planning call (`cruise plan`) on the issue title and body, and post the result as a new **plan-tracking comment**. Nothing is executed. |
| `@cruise fix <feedback>`, `@cruise /fix <feedback>` | **fix** | Revise the most recent *trusted* plan-tracking comment using `<feedback>`, then **edit that same comment in place** with the revised plan. Fails with a clear message if there is no existing plan comment. |
| `@cruise /tell <instruction>` | **tell** | Send an instruction to a live `run` job on this issue. Issue comments only; it does not start a run. |

**Plan resolution** (used by `run` and `exec`): the action looks at every comment on the issue for the last one that contains the `<!-- cruise:plan -->` marker and was posted by a **bot account** (`user.type == "Bot"`) whose login is one of its fixed allowlisted identities (`cruise-agent[bot]` or `github-actions[bot]`); a comment posted under any other identity (including a PAT or custom App token) is not reused, and resolution falls back to the issue's own title + body. If a trusted plan comment exists, its plan content is used, otherwise the issue's own title + body is used. Either way, any text typed after the command word in the comment/issue that triggered this run is appended as a "## Additional instructions from the triggering comment" section.

<a id="typical-workflow"></a>
## Typical workflow

An end-to-end example, planning first:

1. **Open an issue** describing the task. Leave the trigger phrase (`@cruise`) out of the title and body -- see the warning below for why that matters.

   > **Add retry logic to the uploader**
   >
   > The S3 upload occasionally fails on flaky networks. It should retry with exponential backoff instead of failing the whole job.

2. **Ask for a plan**: comment `@cruise plan`. The action runs an LLM planning call and posts a new plan-tracking comment; nothing is executed yet.

3. **Iterate on the plan** as many times as needed: comment `@cruise fix also add a changelog entry and cover the timeout case`. The action edits the *same* tracking comment in place with the revised plan.

4. **Execute it**: comment `@cruise run`. The action creates a session from the (possibly revised) plan and implements it in a worktree. If the successful run leaves branch commits and automatic PR creation is enabled, the action opens a draft PR; a successful no-commit or `create_pr: false` run completes without a PR.

5. **Review and ready it**: if a PR was created, Cruise always opens it as a **draft**, regardless of which command created it. Open the PR, review the diff, then mark it "Ready for review" (or `gh pr ready <number>`) before merging. A successful no-commit or no-PR run has no PR to review.

> [!WARNING]
> If the issue's own title or body already contains the trigger phrase, the `issues: [opened]` event fires immediately. The first token after leading whitespace and punctuation, even across a newline, selects `plan`, `fix`, `exec`, or `run`; only a missing or unknown token defaults to `run`. Keep the trigger phrase out of the issue itself when you want to review a plan first; only type it in a follow-up comment.

Multiple mentions on the same issue queue rather than race each other. It is safe to keep commenting without waiting for each run to finish.

While a `run` job is active, it checks authorized issue comments. Accepted or rejected comments receive an eyes reaction; accepted text is queued for the next agent prompt, not the prompt currently running.

**No response to a tell?** Check that the comment used `/tell`, the author is authorized, and a `run` job for that issue was active. Check the job logs and reaction.

<a id="setup"></a>
## Setup

1. **Install the `cruise-agent` GitHub App** on your repository: [github.com/apps/cruise-agent/installations/new](https://github.com/apps/cruise-agent/installations/new). This is what lets the action authenticate as a scoped bot identity (`cruise-agent[bot]`) instead of the workflow's own `GITHUB_TOKEN` -- see [How authentication works](#how-authentication-works) below. You can skip this step; the action still runs, but falls back to `GITHUB_TOKEN` with the limitations described there.
2. Add an `ANTHROPIC_API_KEY` and/or `OPENAI_API_KEY` secret to your repository (Settings -> Secrets and variables -> Actions). At least one is required.
3. Copy [`examples/cruise.yml`](../../examples/cruise.yml) to `.github/workflows/cruise.yml` (the version below is a condensed excerpt):

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

<a id="providers"></a>
## Providers

This action provisions cruise's default `sdk: jcode` backend (see [below](#sdk-jcode----how-execution-works)); a repository config that names another backend keeps it. jcode ships several dozen providers -- each one just needs its API key, supplied one of three ways:

- **Dedicated inputs** -- `anthropic_api_key` / `openai_api_key` are registered as jcode `anthropic-api` / `openai-api` credentials.
- **`env`** -- any other provider's key (e.g. `KIMI_API_KEY`, `GROQ_API_KEY`, `OPENROUTER_API_KEY`) is exported into the run, where jcode picks it up from the environment.
- **`providers` + `provider_api_keys`** -- for an OpenAI-compatible endpoint jcode does not already know about (a self-hosted gateway or proxy), each `providers` entry becomes a `[providers.<name>]` profile via `jcode provider add`.

```yaml
with:
  providers: |
    {
      "my-gateway": {
        "base_url": "https://gateway.example/v1",
        "model": "my-custom-model",
        "context_window": 200000,
        "default": true
      }
    }
  provider_api_keys: |
    my-gateway=${{ secrets.MY_GATEWAY_KEY }}
```

The profile's key is handed to `jcode provider add --api-key-stdin`, which stores it in an owner-only env file under the job's jcode home -- `config.toml` references it by environment-variable name only, and no key is ever written to a cruise config file.

<a id="provider-profile-schema"></a>
### Provider profile schema

Each key of `providers` is a profile name (ASCII letters, numbers, `-` and `_`, starting with a letter or number, at most 64 characters) mapping to an object with:

- **`base_url`** (required) -- the endpoint URL.
- **`model`** (required) -- the profile's default model id.
- **`context_window`** (optional) -- a number.
- **`auth`** (optional) -- `bearer` (the default) or `api-key`.
- **`auth_header`** (optional) -- the header name; requires `auth: api-key`.
- **`provider_routing`** (optional, boolean) -- for OpenRouter-style gateways.
- **`no_auth`** (optional, boolean) -- the endpoint takes no credential at all (a local proxy, a dev gateway). A `no_auth` entry must **not** appear in `provider_api_keys`; `provider_api_keys` may be left empty entirely once every `providers` entry is `no_auth`.
- **`default`** (optional, boolean) -- make the profile jcode's startup default provider/model. At most one entry may set it.

Any key not on this list is rejected with an error naming it -- a typo would otherwise be silently dropped.

<a id="built-in-providers"></a>
### Built-in providers

| Provider | jcode CLI credential provider | Env var(s) |
|---|---|---|
| Anthropic | `anthropic-api` | `ANTHROPIC_API_KEY` (dedicated `anthropic_api_key` input) |
| OpenAI | `openai-api` | `OPENAI_API_KEY` (dedicated `openai_api_key` input) |
| Kimi for Coding | `kimi` | `KIMI_API_KEY` (via `env`) |
| Google Gemini | `gemini-api` | `GEMINI_API_KEY` (via `env`) |
| Groq | `groq` | `GROQ_API_KEY` (via `env`) |
| Mistral AI | `mistral` | `MISTRAL_API_KEY` (via `env`) |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY` (via `env`) |
| xAI (Grok) | `xai` | `XAI_API_KEY` (via `env`) |
| Moonshot AI | `moonshot-ai` | `MOONSHOT_API_KEY` (via `env`) |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` (via `env`) |

The identifiers in this table are for CLI credential provisioning; `jcode login --help` lists accepted IDs. Custom endpoint profiles are provisioned through `providers` / `provider_api_keys` with `jcode provider add`.

<a id="kimi-for-coding-example"></a>
### Kimi for Coding example

[Kimi for Coding](https://api.kimi.com/coding/) uses the jcode CLI credential provider `kimi`, authenticated with `KIMI_API_KEY`. Supply a compatible Cruise model through the action's `model` input or the repository workflow / `.cruise/default.yaml`.

```yaml
- uses: smartcrabai/cruise@v1
  with:
    # Set model to a route available for this credential, or configure it in
    # the repository workflow / .cruise/default.yaml.
    env: |
      KIMI_API_KEY=${{ secrets.KIMI_API_KEY }}
```

<a id="sdk-jcode----how-execution-works"></a>
## sdk: jcode -- how execution works

Cruise resolves the backend from the repository workflow config and applicable checkout project defaults. A repository workflow config that sets `command:` or `sdk: claude` remains in effect; provide that backend's own CLI and credentials if needed.

- **OpenAI service tier** defaults to `off` when neither the workflow nor process environment defines `JCODE_OPENAI_SERVICE_TIER`; an explicit value is respected.
- **Model inputs** (`model` / `plan_model`) override the corresponding Cruise settings. Every actual jcode prompt requires an effective Cruise model in `provider/model[:effort]` or bare-model form. `model` may come from the action input, repository workflow, or checkout `.cruise/default.yaml`; the jcode home default and `JCODE_MODEL` do not substitute. `plan_model` is used for planning calls and otherwise falls back to `model`. The optional effort suffix supports `low|medium|high|xhigh|max` and aliases documented in [SDK mode](sdk.md#sdk-mode).

<a id="zero-config-default"></a>
### Zero-config default

When `config` is empty, there is no root workflow config, and there are no YAML files directly under `.cruise/`, this action generates a `write-tests -> implement` config. The applicable checkout project defaults or action model input must supply a model for jcode prompts.

<a id="config-resolution"></a>
## config resolution

- **`config` input set** -- resolved to an absolute path and used by `run`/`plan`/`fix`.
- **`config` input empty, with an existing repository config** -- cruise's own resolver selects the file.

In either case, `model`/`plan_model` inputs (`CRUISE_MODEL`/`CRUISE_PLAN_MODEL` environment overrides applied by Cruise itself) take precedence regardless of whether the config file sets them.

Avoid `option:` steps in a config used for CI -- they prompt interactively and there is no terminal attached in Actions.

<a id="env-input"></a>
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

Blank lines and lines starting with `#` are ignored. Each value is masked (`::add-mask::`) before being exported. Reserved names -- `GITHUB_TOKEN`, `GH_TOKEN`, `PATH`, `HOME`, `SHELL`, git identity vars (`GIT_AUTHOR_*`/`GIT_COMMITTER_*`), `JCODE_HOME`, `XDG_DATA_HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME`, and names prefixed with `CRUISE_`/`GITHUB_`/`ACTIONS_`/`RUNNER_` -- are skipped with a `::warning::` instead of being overridden because the action manages them. `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` are not reserved and can be passed through `env`, instead of or alongside their dedicated inputs.

<a id="exec-caveats"></a>
## exec caveats

`exec` pushes **directly to the default branch**, with no PR and no review step. This:

- **Interacts badly with branch protection.** If the default branch requires PRs, status checks, or reviews before merging, cruise's direct push will simply fail (surfaced as a failed run). `exec` is meant for repositories that intentionally allow direct pushes to their default branch, or bypass rules for the actor/token cruise uses.
- **Is advanced/opt-in.** There is no undo beyond `git revert`. Prefer `run` (which opens a draft PR you can review) unless you specifically want unattended direct pushes.
- Skips the commit+push step entirely (and reports success) if `cruise exec` produced no file changes.

<a id="how-authentication-works"></a>
## How authentication works

By default (`github_token` input left empty), the action tries to authenticate as the `cruise-agent` GitHub App instead of using the workflow's own `GITHUB_TOKEN`:

1. With `permissions: id-token: write` granted, GitHub Actions gives the job a short-lived OIDC token identifying the workflow, repository, and run.
2. The OIDC token is exchanged for a **repository-scoped, short-lived cruise-agent App installation token**. Only a token scoped to the calling repository is issued.
3. cruise runs and pushes commits authenticated as `cruise-agent[bot]` using that token. Commits created by the action also add the user who triggered the mention as a `Co-authored-by` trailer using that user's GitHub-provided noreply address.

If the App is not installed on the repository, the OIDC token cannot be obtained (for example, `id-token: write` was not granted), or the exchange service is unreachable, the action **falls back to the workflow's `GITHUB_TOKEN`**. cruise still runs, but draft PRs created with `GITHUB_TOKEN` do not trigger other `on: pull_request` workflows, and commits use `github-actions[bot]` as the author/committer.

<a id="bring-your-own-token"></a>
## Bring your own token

Set `github_token` explicitly (e.g. to a PAT or a token from your own GitHub App) to skip the OIDC exchange entirely. The action uses that token for every GitHub API call and push.

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    github_token: ${{ secrets.MY_PAT }}
```

<a id="self-hosting-the-token-exchange"></a>
## Self-hosting the token exchange

To run a token-exchange service with your own GitHub App and Worker, point the action at it with `token_exchange_url`. See [`../../token-exchange/README.md`](../../token-exchange/README.md) for the source and deployment instructions.

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    token_exchange_url: https://your-worker.your-subdomain.workers.dev/token
```

Set `token_exchange_url: ""` to disable the exchange outright and always use `github_token`/`GITHUB_TOKEN`.

In either case, the action posts a tracking comment when it starts and rewrites it with the outcome when it finishes. Run logs are never copied into issue comments.

<a id="inputs"></a>
## Inputs

| Input | Default | Description |
|---|---|---|
| `anthropic_api_key` | *(empty)* | Anthropic API key. At least one of `anthropic_api_key`, `openai_api_key`, `provider_api_keys`, `providers`, or `env` must be non-empty. |
| `openai_api_key` | *(empty)* | OpenAI API key. At least one of `anthropic_api_key`, `openai_api_key`, `provider_api_keys`, `providers`, or `env` must be non-empty. |
| `github_token` | *(empty)* | Token for GitHub API calls (permission checks, comments, PRs, pushes). Empty (default) tries the cruise-agent App OIDC token exchange first, falling back to the workflow's `GITHUB_TOKEN`; set explicitly to skip the exchange and use that token instead. See [How authentication works](#how-authentication-works). |
| `token_exchange_url` | `https://cruise-token-exchange.smartcrab.ai/token` | URL of the token-exchange service. Empty disables the exchange (always falls back to `github_token`/`GITHUB_TOKEN`). See [Self-hosting the token exchange](#self-hosting-the-token-exchange). |
| `trigger_phrase` | `@cruise` | Phrase that must appear (word-boundary match) to trigger a run. For the `issues` event, matching uses the issue title and body concatenated, so a phrase in the title is effective. |
| `cruise_version` | `latest` | cruise release to install (`latest` or a tag). |
| `jcode_version` | `latest` | jcode release to install via jcode's own installer (`latest` or a tag). Ignored when `jcode` is already on PATH. |
| `config` | *(empty)* | Path to a cruise workflow config YAML in your repo, used by `run`/`plan`/`fix` (sets `CRUISE_CONFIG`). Empty lets cruise's own resolver pick a config from the checkout, or its built-in `tdd` workflow. No effect on `exec`. |
| `model` | *(empty)* | Overrides `CRUISE_MODEL`; for `sdk: jcode`, use `provider/model[:effort]`, a bare model, or leave the override unset. Every actual jcode prompt must resolve an effective Cruise model from this input, the workflow config, checkout `.cruise/default.yaml`, or (for planning prompts) `plan_model`/`CRUISE_PLAN_MODEL`. jcode's configured default and `JCODE_MODEL` do not substitute. |
| `flags` | *(empty)* | Workflow flag values for `run` (blank lines and lines starting with `#` are ignored). Each remaining line is passed as one `--flag` value and may be a bare `NAME`, a comma-separated list, or explicit `NAME=on\|off\|true\|false` assignments. Flags absent from the input are off; no flag is required. No effect on `exec`, `plan` or `fix`. |
| `plan_model` | *(empty)* | Overrides `CRUISE_PLAN_MODEL` (the `plan`/`fix` commands' planning step), same format as `model`. |
| `providers` | *(empty)* | JSON object keyed by jcode provider-profile name. Each entry requires `base_url` and `model`; optional `context_window`, `auth`, `auth_header`, `provider_routing`, `no_auth`, `default`. Each entry becomes a `[providers.<name>]` profile via `jcode provider add`. See [Providers](#providers) for the full schema. |
| `provider_api_keys` | *(empty)* | One `profile-name=API key` per line for `providers`; blank/comment lines ignored and the first `=` separates the key. Each key is handed to `jcode provider add --api-key-stdin`. Required for every `providers` entry that isn't `no_auth: true`; may be left empty when every entry is `no_auth`. |
| `env` | *(empty)* | Extra `KEY=VALUE` lines exported (masked) into the cruise process. Reserved names are skipped with a warning. |
| `allowed_bots` | *(empty)* | Comma-separated bot logins (without `[bot]`) allowed to trigger cruise, or `*` for any bot. Empty blocks all bots. |
| `git_user_name` | *(empty)* | git `user.name` for commits this action/cruise creates. Empty resolves to `cruise-agent[bot]` when the run used the App token, otherwise `github-actions[bot]`. |
| `git_user_email` | *(empty)* | git `user.email` for those commits. Empty resolves to match `git_user_name`'s default. Commits still add the triggering user as a `Co-authored-by` trailer when GitHub's event payload includes their login and numeric user id. |

<a id="outputs"></a>
## Outputs

| Output | Description |
|---|---|
| `command` | `"run"`, `"exec"`, `"plan"`, or `"fix"` -- the command parsed from the mention (empty if the gate skipped the run). |
| `session_id` | The ID of the cruise session created by the command. |
| `pr_url` | URL of the pull request cruise opened (the `run` command only); empty when no PR was created. |
| `commit_url` | URL of the commit cruise pushed to the default branch (the `exec` command only). |
| `plan_comment_url` | URL of the plan-tracking comment cruise posted or edited (the `plan`/`fix` commands only). |
| `conclusion` | `success`, `failure`, or `skipped` (the mention did not match, or the actor was not authorized). |
| `used_app` | `"true"` if the run authenticated with a cruise-agent App installation token; `"false"` if it used `github_token`/`GITHUB_TOKEN`. |

<a id="security"></a>
## Security

- **Only repository collaborators can trigger cruise.** The action calls the GitHub collaborator-permission API for the commenting/mentioning user and requires write access (the API reports the `maintain` role as `write`, so maintainers qualify; `triage` and `read` do not). Bot actors are rejected unless explicitly added to `allowed_bots`.
- **Plan comments are trust-checked, not just marker-checked.** `run`/`exec`/`fix` only treat a comment as an authoritative plan source if it was posted by `cruise-agent[bot]`/`github-actions[bot]` *and* contains the plan marker. Without this, a commenter without write access could post a fake `<!-- cruise:plan -->` comment that a maintainer's later `@cruise run` would execute unreviewed (only the *mention itself* requires an authorized actor -- any other comment on a public issue does not).
- **The token exchange issues repository-scoped tokens only.** Workflows can obtain a token scoped only to the repository in which they are running. `permissions: id-token: write` only lets the job request an OIDC token from GitHub; it does not itself grant GitHub API access.
- **Unattended execution.** cruise does not show per-action confirmation prompts. A successful prompt injection (see below) has the same blast radius as the workflow's GitHub token and runner. Grant only the required `permissions:` (`contents: write`, `pull-requests: write`, `issues: write`, plus `id-token: write` for App token exchange), and treat provider API keys, `GITHUB_TOKEN`, and any App token as CI secrets with repository write access.
- **Prompt injection.** Issue bodies and fix feedback are attacker-controlled text embedded in the model prompt. Before GitHub-sourced text becomes planning input, the action removes hidden instruction vectors. This is a mitigation, not a guarantee; visible plain-text instructions cannot be filtered. Do not grant secrets or permissions beyond what would be acceptable if the agent did everything the issue text asked.
- **Run logs are never posted back to the issue.** The failure comment links to the Actions run instead of quoting log output. Agent output can contain anything the model was coaxed into printing (including environment values), and GitHub's secret masking only applies to the Actions log viewer -- not to text re-posted through the API -- so copying logs into a public comment would be an exfiltration channel. Logs stay on the run page, which follows your repository's access controls.
- **The installer is fetched over TLS but not checksum-verified.** The action installs cruise and jcode through release installer scripts (`curl | sh`), but the scripts themselves are not checksummed. A compromised download endpoint could run attacker code with the job secrets. Pin `cruise_version`/`jcode_version` to tags to avoid silently tracking `latest`.
- **PRs opened via `GITHUB_TOKEN` don't trigger other workflows.** This is a deliberate GitHub Actions anti-recursion rule, and it only applies to the plain `GITHUB_TOKEN` fallback path -- PRs/pushes made with the cruise-agent App's installation token (the default, when the App is installed) trigger `on: pull_request` workflows normally, since GitHub treats App-authenticated actions as coming from a distinct actor. If you're on the `GITHUB_TOKEN` fallback and need CI to run on cruise's PRs, either install the App, use your own PAT/GitHub App token via `github_token`, or add a step that closes/reopens the PR.

<a id="troubleshooting"></a>
## Troubleshooting

- **Nothing happens after mentioning `@cruise`.** Check the workflow run list for a skipped/no-op run: the gate step logs why it declined (event type, action, missing trigger phrase, PR comment, or insufficient actor permission).
- **"'anthropic_api_key', 'openai_api_key', 'provider_api_keys', 'providers', and 'env' are all empty".** Set at least one dedicated key, `providers`/`provider_api_keys` for a generated profile, or a provider key via `env` (e.g. `KIMI_API_KEY=...`).
- **"actor '&lt;login&gt;' has insufficient permission: '&lt;permission&gt;'".** The commenter needs `write`, `maintain`, or `admin` access to the repository.
- **"No existing plan comment found" (fix).** Run `@cruise plan` first; `fix` only edits an existing plan-tracking comment, it doesn't create one.
- **"cruise completed but no pull request was created" (run).** Check `permissions: pull-requests: write` and branch-protection/repository rules.
- **`exec`'s push fails.** Usually branch protection on the default branch -- see [exec caveats](#exec-caveats).
- **Model resolution errors.** Ensure every actual jcode prompt resolves an effective Cruise model. The jcode configured default and `JCODE_MODEL` do not substitute. Set the `model`/`plan_model` inputs or configure them in the workflow or checkout `.cruise/default.yaml`, as appropriate for the prompt.
- **Run always falls back to `GITHUB_TOKEN` (`used_app` output is `false`).** Check, in order: the `cruise-agent` App is installed on this repository ([install link](https://github.com/apps/cruise-agent/installations/new)); the workflow grants `permissions: id-token: write`; `token_exchange_url` is not empty and reachable. The `token` step's log line explains which of these failed.
- **Self-hosted runners** need `git`, `curl`, `jq`, `python3`, and the `gh` CLI on `PATH` (all preinstalled on GitHub-hosted runners).

<a id="faq"></a>
### FAQ

- **Why did my brand-new issue immediately turn into a PR instead of posting a plan first?** Its title or body already contained the trigger phrase, so the `issues: [opened]` event fired immediately. To review a plan first, leave the trigger phrase out of the issue itself and comment `@cruise plan` afterward.
- **The pull request cruise opened won't merge / doesn't show up as ready.** PRs from `run` are always opened as **drafts**. Review the diff, then mark it "Ready for review" yourself; see [Typical workflow](#typical-workflow).
- **My run timed out.** Start with `timeout-minutes: 60` as in [`examples/cruise.yml`](../../examples/cruise.yml); increase it further for slower test suites.
- **My second `@cruise` comment on the same issue seems stuck, not running.** It is queued, not lost. Check the Actions run list; the earlier run may still be in progress.
