# cruise GitHub Action

Mention `@cruise` (configurable) on a GitHub Issue to have cruise plan, implement, and open a draft PR inside GitHub Actions -- similar in spirit to [`anthropics/claude-code-action`](https://github.com/anthropics/claude-code-action), but driving cruise's own plan -> implement -> PR workflow, always through the `sdk: pi` backend (`pi_agent_rust`, in-process -- no `claude` CLI install).

This action has **no pull-request mode**: it only reacts to `issues` (opened) and `issue_comment` (created) events. A comment made on a pull request is always ignored (PRs are "issues" at the GitHub API level, but this action explicitly excludes them).

## Commands

The first word after the `@cruise` mention (with or without a leading `/`, case-insensitive, trailing punctuation like `.`/`,`/`:` stripped before matching) selects what happens. Anything else -- including no word at all -- is treated as `run`.

| Mention | Command | What happens |
|---|---|---|
| `@cruise`, `@cruise run <request>`, `@cruise /run` | **run** | Resolve the plan (see below; any text typed after `run` is appended as extra instructions), create a session from it verbatim (no LLM planning call), execute it in a worktree, push a branch, and open a **draft pull request**. |
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

### Typical workflow

```
@cruise plan add retry logic to the uploader
    -> posts a plan-tracking comment

@cruise fix also add a changelog entry
    -> edits the same comment with a revised plan

@cruise run
    -> creates a session from the (revised) plan and opens a draft PR
```

You can also skip straight to `@cruise run <request>` (or just `@cruise <request>`) on a fresh issue with no plan comment yet -- the issue's title + body becomes the plan directly (with `<request>`, if any, appended as additional instructions), with no separate planning call.

## Setup

1. **Install the `cruise-agent` GitHub App** on your repository: [github.com/apps/cruise-agent/installations/new](https://github.com/apps/cruise-agent/installations/new). This is what lets the action authenticate as a scoped bot identity (`cruise-agent[bot]`) instead of the workflow's own `GITHUB_TOKEN` -- see [How authentication works](#how-authentication-works) below. You can skip this step; the action still runs, but falls back to `GITHUB_TOKEN` with the limitations described there.
2. Add an `ANTHROPIC_API_KEY` and/or `OPENAI_API_KEY` secret to your repository (Settings -> Secrets and variables -> Actions). At least one is required.
3. Copy [`examples/cruise.yml`](../examples/cruise.yml) to `.github/workflows/cruise.yml`:

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
       timeout-minutes: 30
       permissions:
         contents: write
         pull-requests: write
         issues: write
         id-token: write # needed for the cruise-agent App token exchange; see below
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

## sdk: pi -- how execution works

This action always forces `CRUISE_SDK=pi` in the environment before invoking cruise, regardless of what any config file says (`command:`/`sdk:` in a repo's own `cruise.yaml` are overridden). This means:

- **No `claude` CLI is installed.** cruise drives `pi_agent_rust` directly, in-process.
- **Authentication** is resolved entirely by pi, in this order: an explicit key (not exposed here) > pi's stored `~/.pi/agent/auth.json` OAuth/Bearer credentials (only relevant on a persistent self-hosted runner where someone ran `pi login` ahead of time) > the `ANTHROPIC_API_KEY`/`OPENAI_API_KEY` env vars this action sets from the `anthropic_api_key`/`openai_api_key` inputs. At least one of the two inputs is required -- the gate step fails clearly if both are empty.
- **Model selection** (`model`/`plan_model` inputs, mapped to `CRUISE_MODEL`/`CRUISE_PLAN_MODEL`) uses pi's model-reference format, not seher mode keys:
  - `"provider/model"`, optionally with `:thinking` (e.g. `openai-codex/gpt-5.5:xhigh`) -- selects that provider and model explicitly.
  - `"model"` (no `/`) -- pi searches its own model registry for that id.
  - Empty (default) -- pi auto-selects a provider/model from its built-in preference order, picking the first one with usable credentials.
- **Custom endpoints / providers** (`pi_models_json` input): paste the raw contents of a pi `models.json` file (OpenAI-compatible endpoints, custom providers, registry overrides). The action writes it to `$RUNNER_TEMP/pi-agent/models.json` and points `PI_CODING_AGENT_DIR` at that directory for the run:

  ```yaml
  - uses: smartcrabai/cruise@v1
    with:
      openai_api_key: ${{ secrets.MY_COMPATIBLE_ENDPOINT_KEY }}
      pi_models_json: |
        {
          "openai": {
            "baseUrl": "https://my-openai-compatible-endpoint.example.com/v1",
            "models": ["my-custom-model"]
          }
        }
  ```

### Zero-config default: pi auto-selects the model

The default (no `model`/`plan_model` input) is pi's own auto-selection -- no model configuration is required to get started. This matters because cruise's *built-in* default workflow (used when no config file exists at all) hardcodes `model: sonnet` / `plan_model: opus` as literal strings, which under `sdk: pi` would be interpreted as bare pi model-registry ids rather than seher mode keys -- and pi has no id named exactly `sonnet`/`opus` (real ids look like `claude-sonnet-4-6`), so relying on that raw built-in default would fail to resolve a model. To avoid this, when `config` is empty **and** the repository has no config of its own, this action generates a default config itself (see [config resolution](#config-resolution) below) that mirrors cruise's `write-tests -> implement` workflow but deliberately omits `model`/`plan_model`, letting pi auto-select based on whichever of `anthropic_api_key`/`openai_api_key` is set. Set the `model`/`plan_model` inputs explicitly (in pi's reference format) if you want a specific model instead.

## config resolution

- **`config` input set** -- resolved to an absolute path and exported as `CRUISE_CONFIG`. Used by the `run`/`plan`/`fix` commands.
- **`config` input empty, and the repository already has its own config** (`cruise.yaml`/`cruise.yml`/`.cruise.yaml`/`.cruise.yml` at the checkout root, or any YAML file under `.cruise/`) -- `CRUISE_CONFIG` is left unset entirely and cruise's own resolver picks that file up.
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

Blank lines and lines starting with `#` are ignored. Each value is masked (`::add-mask::`) before being exported. Reserved names -- `GITHUB_TOKEN`, `GH_TOKEN`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `CRUISE_SDK`, `CRUISE_CONFIG`, `PI_CODING_AGENT_DIR`, `PATH`, `HOME`, the git identity vars, the `XDG_*` vars, and anything prefixed `GITHUB_`/`ACTIONS_`/`RUNNER_` -- are skipped with a `::warning::` instead of being overridden, since the action itself manages them.

## exec caveats

`exec` pushes **directly to the default branch**, with no PR and no review step. This:

- **Interacts badly with branch protection.** If the default branch requires PRs, status checks, or reviews before merging, cruise's direct push will simply fail (surfaced as a failed run). `exec` is meant for repositories that intentionally allow direct pushes to their default branch, or bypass rules for the actor/token cruise uses.
- **Is advanced/opt-in.** There is no undo beyond `git revert`. Prefer `run` (which opens a draft PR you can review) unless you specifically want unattended direct pushes.
- Skips the commit+push step entirely (and reports success) if `cruise exec` produced no file changes.

## How authentication works

By default (`github_token` input left empty), the action tries to authenticate as the `cruise-agent` GitHub App instead of using the workflow's own `GITHUB_TOKEN`:

1. With `permissions: id-token: write` granted, GitHub Actions gives the job a short-lived OIDC token identifying the workflow, repository, and run.
2. The `token` step exchanges that OIDC token for a **repository-scoped, short-lived cruise-agent App installation token** by calling the `token_exchange_url` service (`POST` with `Authorization: Bearer <OIDC token>`, no body). The exchange service verifies the OIDC token's `repository` claim server-side and only ever issues a token scoped to that repository's installation -- it cannot mint a token for a repository the calling workflow doesn't belong to.
3. cruise runs and pushes commits authenticated as `cruise-agent[bot]` using that token.
4. After the run finishes (success or failure), the action revokes the token (`DELETE /installation/token`) so it can't be reused past the job's lifetime.

If the App isn't installed on the repository (the exchange returns 404), the OIDC token can't be obtained (e.g. `id-token: write` wasn't granted), or the exchange service is unreachable, the action **falls back to the workflow's `GITHUB_TOKEN`** and logs why (a `::notice::` with the App install link for the "not installed" case, a `::warning::` otherwise). cruise still runs in that case, but with two `GITHUB_TOKEN`-specific limitations: draft PRs it opens won't trigger other `on: pull_request` workflows (see [Security](#security)), and commits are attributed to `github-actions[bot]` instead of `cruise-agent[bot]`.

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
| `anthropic_api_key` | *(empty)* | Anthropic API key for pi. At least one of `anthropic_api_key`/`openai_api_key` is required. |
| `openai_api_key` | *(empty)* | OpenAI API key for pi. At least one of `anthropic_api_key`/`openai_api_key` is required. |
| `github_token` | *(empty)* | Token for GitHub API calls (permission checks, comments, PRs, pushes). Empty (default) tries the cruise-agent App OIDC token exchange first, falling back to the workflow's `GITHUB_TOKEN`; set explicitly to skip the exchange and use that token instead. See [How authentication works](#how-authentication-works). |
| `token_exchange_url` | *(cruise-agent's hosted exchange)* | URL of the token-exchange service. Empty disables the exchange (always falls back to `github_token`/`GITHUB_TOKEN`). See [Self-hosting the token exchange](#self-hosting-the-token-exchange). |
| `trigger_phrase` | `@cruise` | Phrase that must appear (word-boundary match) in the body to trigger a run. |
| `cruise_version` | `latest` | cruise release to install (`latest` or a tag like `v0.1.68`). Requires v0.1.68+. |
| `config` | *(empty)* | Path to a cruise workflow config YAML in your repo, used by `run`/`plan`/`fix` (sets `CRUISE_CONFIG`). Empty lets cruise's own resolver pick a config from the checkout, or its built-in default. No effect on `exec`. |
| `model` | *(empty)* | Overrides `CRUISE_MODEL`, in pi's model-reference format (`provider/model[:thinking]`, a bare model id, or empty for auto-select). |
| `plan_model` | *(empty)* | Overrides `CRUISE_PLAN_MODEL` (the `plan`/`fix` commands' planning step), same format as `model`. |
| `pi_models_json` | *(empty)* | Raw contents of a pi `models.json` file. When set, written to `$RUNNER_TEMP/pi-agent/models.json` with `PI_CODING_AGENT_DIR` pointed at it. |
| `env` | *(empty)* | Extra `KEY=VALUE` lines exported (masked) into the cruise process. Reserved names are skipped with a warning. |
| `allowed_bots` | *(empty)* | Comma-separated bot logins (without `[bot]`) allowed to trigger cruise, or `*` for any bot. Empty blocks all bots. |
| `git_user_name` | *(empty)* | git `user.name` for commits this action/cruise creates. Empty resolves to `cruise-agent[bot]` when the run used the App token, otherwise `github-actions[bot]`. |
| `git_user_email` | *(empty)* | git `user.email` for those commits. Empty resolves to match `git_user_name`'s default. |

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
- **"both 'anthropic_api_key' and 'openai_api_key' are empty".** Set at least one as a secret and pass it in.
- **"actor has insufficient permission".** The commenter needs `write`, `maintain`, or `admin` access to the repository.
- **"No existing plan comment found" (fix).** Run `@cruise plan` first; `fix` only edits an existing plan-tracking comment, it doesn't create one.
- **"cruise completed but no pull request was created" (run).** cruise ran (and may have pushed a branch), but `gh pr create` failed. Check that the workflow grants `permissions: pull-requests: write` and that branch protection / repository rules allow creating PRs from the pushed branch.
- **`exec`'s push fails.** Usually branch protection on the default branch -- see [exec caveats](#exec-caveats).
- **Model resolution errors.** With no `config` input and no repository config, this action already generates a `model`/`plan_model`-free default so pi auto-selects (see [zero-config default](#zero-config-default-pi-auto-selects-the-model)). If you *do* have your own `config` and see this, set the `model`/`plan_model` inputs explicitly in pi's reference format, or remove any leftover seher-style mode keys (e.g. `sonnet`/`opus`) from your config's `model:`/`plan_model:`.
- **Run always falls back to `GITHUB_TOKEN` (`used_app` output is `false`).** Check, in order: the `cruise-agent` App is installed on this repository ([install link](https://github.com/apps/cruise-agent/installations/new)); the workflow grants `permissions: id-token: write`; `token_exchange_url` is not empty and reachable. The `token` step's log line explains which of these failed.
- **Self-hosted runners** need `git`, `curl`, `jq`, `python3`, and the `gh` CLI on `PATH` (all preinstalled on GitHub-hosted runners).
