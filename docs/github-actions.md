# cruise GitHub Action

Trigger cruise from a GitHub Issue or Pull Request by mentioning `@cruise` (configurable) in a comment, an issue body, or a PR review. cruise then plans, implements, and opens a draft PR (or pushes directly to the PR branch it was mentioned on) inside GitHub Actions -- similar in spirit to [`anthropics/claude-code-action`](https://github.com/anthropics/claude-code-action), but driving cruise's own plan -> implement -> PR workflow.

## Setup

1. **Install the `cruise-agent` GitHub App** on your repository: [github.com/apps/cruise-agent/installations/new](https://github.com/apps/cruise-agent/installations/new). This is what lets the action authenticate as a scoped bot identity (`cruise-agent[bot]`) instead of the workflow's own `GITHUB_TOKEN` -- see [How authentication works](#how-authentication-works) below. You can skip this step; the action still runs, but falls back to `GITHUB_TOKEN` with the limitations described there.
2. Add an `ANTHROPIC_API_KEY` secret to your repository (Settings -> Secrets and variables -> Actions).
3. Copy [`examples/cruise.yml`](../examples/cruise.yml) to `.github/workflows/cruise.yml`:

   ```yaml
   name: Cruise

   on:
     issue_comment:
       types: [created]
     issues:
       types: [opened]
     pull_request_review_comment:
       types: [created]
     pull_request_review:
       types: [submitted]

   jobs:
     cruise:
       if: |
         (github.event_name == 'issue_comment' && contains(github.event.comment.body, '@cruise')) ||
         (github.event_name == 'issues' && (contains(github.event.issue.title, '@cruise') || contains(github.event.issue.body, '@cruise'))) ||
         (github.event_name == 'pull_request_review_comment' && contains(github.event.comment.body, '@cruise')) ||
         (github.event_name == 'pull_request_review' && contains(github.event.review.body, '@cruise'))
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

4. Comment `@cruise <what you want done>` on an issue or PR (or open an issue whose title/body mentions `@cruise`).

The workflow-level `if:` is only a coarse pre-filter (so unrelated events don't spin up a runner); the action independently re-checks the trigger phrase with a strict word-boundary match and verifies the commenter's permissions before doing anything, so it is safe even if the pre-filter is removed.

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

## How it works

| Where `@cruise` was mentioned | What happens |
|---|---|
| Issue comment / new issue | `cruise plan` (or `cruise --plan stdin --skip-planning` if `skip_planning: true`) creates a session from the issue title + body + recent comments, then `cruise run` executes it in a worktree, pushes a branch, and opens a **draft pull request**. |
| PR comment / PR review | `gh pr checkout` checks out the PR's branch, then `cruise exec` runs the same task directly on it (no new plan, no new branch). Any resulting changes are committed and **pushed straight to the PR branch**. |

In both cases the action posts a tracking comment when it starts and rewrites it with the outcome when it finishes -- success with links, or failure with a link to the run. Run logs are deliberately **never** copied into the comment (see [Security](#security)).

## Inputs

| Input | Default | Description |
|---|---|---|
| `anthropic_api_key` | *(required)* | API key for the `claude` CLI that cruise drives. |
| `github_token` | *(empty)* | Token for GitHub API calls (permission checks, comments, PRs, pushes). Empty (default) tries the cruise-agent App OIDC token exchange first, falling back to the workflow's `GITHUB_TOKEN`; set explicitly to skip the exchange and use that token instead. See [How authentication works](#how-authentication-works). |
| `token_exchange_url` | *(cruise-agent's hosted exchange)* | URL of the token-exchange service. Empty disables the exchange (always falls back to `github_token`/`GITHUB_TOKEN`). See [Self-hosting the token exchange](#self-hosting-the-token-exchange). |
| `trigger_phrase` | `@cruise` | Phrase that must appear (word-boundary match) in the body to trigger a run. |
| `cruise_version` | `latest` | cruise release to install (`latest` or a tag like `v0.1.66`). |
| `config` | *(empty)* | Path to a cruise workflow config YAML in your repo. Empty uses an auto-generated default (see below). |
| `skip_planning` | `false` | Issue mode only: use the mention body verbatim as the plan instead of an LLM planning call. |
| `model` | *(empty)* | Overrides `CRUISE_MODEL` (implementation steps' model). |
| `plan_model` | *(empty)* | Overrides `CRUISE_PLAN_MODEL` (planning step's model). |
| `claude_args` | `--dangerously-skip-permissions` | Extra arguments appended to `claude` in the **auto-generated** default config only; ignored when `config` is set. Split on whitespace only -- shell-style quoting is not honored (`--foo "bar baz"` becomes three arguments), so use a custom `config` for arguments containing spaces. |
| `allowed_bots` | *(empty)* | Comma-separated bot logins (without `[bot]`) allowed to trigger cruise, or `*` for any bot. Empty blocks all bots. |
| `git_user_name` | *(empty)* | git `user.name` for commits this action/cruise creates. Empty resolves to `cruise-agent[bot]` when the run used the App token, otherwise `github-actions[bot]`. |
| `git_user_email` | *(empty)* | git `user.email` for those commits. Empty resolves to match `git_user_name`'s default. |

## Outputs

| Output | Description |
|---|---|
| `session_id` | The cruise session ID that was created and run. |
| `pr_url` | URL of the pull request cruise opened (issue mode) or updated (PR mode). |
| `conclusion` | `success`, `failure`, or `skipped` (mention didn't match, or actor wasn't authorized). |
| `used_app` | `"true"` if the run authenticated with a cruise-agent App installation token, `"false"` if it used `github_token`/`GITHUB_TOKEN`, or an empty string if the gate step skipped the run before the `token` step ran (mention didn't match / actor not authorized). |

## Custom config

By default the action generates a minimal CI config that mirrors cruise's own built-in default workflow (`write-tests` -> `implement`, using `claude --model {model} -p`), with `claude_args` appended. If your repository already has a cruise config you use locally (for example one with a `test`/`lint` step, or `sdk: seher`), point the action at it instead:

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    config: .github/cruise-ci.yaml
```

The path is resolved relative to the checked-out repository root (or used as-is if absolute). When `config` is set, `model` and `plan_model` still apply, via the `CRUISE_MODEL`/`CRUISE_PLAN_MODEL` environment variable overrides; `claude_args` does not -- it only ever modifies the auto-generated config's `command:` list, so put your own flags directly in your config's `command:` instead.

Avoid `option:` steps in a config used for CI -- they prompt interactively and there is no terminal attached in Actions.

**PR mode passes the task differently than issue mode.** When `@cruise` is mentioned on a PR (comment, review, or review comment), the action runs `cruise exec`, which binds the whole task (title, description, comments, and the triggering request) to `{input}` and leaves `{plan}` **empty** -- there is no planning step in PR mode. The auto-generated default config already accounts for this: it uses a separate single-step PR config (`implement: prompt: "{input}"`) instead of the `{plan}`-based issue-mode config. If you point `config` at your own file, remember that the **same file is used for both modes** -- if your config's steps reference `{plan}`, they will receive an empty string when triggered from a PR. Reference `{input}` in any step that needs to work from a PR mention.

## Security

- **Only repository collaborators can trigger cruise.** The action calls the GitHub collaborator-permission API for the commenting/mentioning user and requires write access (the API reports the `maintain` role as `write`, so maintainers qualify; `triage` and `read` do not). Bot actors are rejected unless explicitly added to `allowed_bots`.
- **The token exchange issues repository-scoped tokens only.** The exchange service validates the `repository` claim embedded in the caller's GitHub Actions OIDC token server-side before minting an installation token, so a workflow can only ever obtain a token scoped to the repository it is actually running in -- never another repository the App happens to be installed on. `permissions: id-token: write` only lets the job *request* that OIDC token from GitHub; it grants no GitHub API access by itself.
- **`--dangerously-skip-permissions` is the default `claude_args` value.** It lets `claude` edit files and run shell commands without per-action confirmation prompts, which is required for unattended CI use -- but it also means a successful prompt-injection (see below) has the same blast radius as the workflow's own GitHub token and runner. Only grant the workflow the `permissions:` it needs (`contents: write`, `pull-requests: write`, `issues: write`, plus `id-token: write` if you want the App token exchange), and treat the Anthropic API key, `GITHUB_TOKEN`, and any App token this action obtains as you would any other CI secret with write access to your repository.
- **Prompt injection.** Issue/PR bodies and comments are attacker-controlled text that gets embedded in the prompt sent to the model. The action strips hidden-instruction vectors -- HTML comments, `<img>` tags (alt-text payloads), and zero-width/bidi-control Unicode characters -- and prepends a note telling the model to treat that content as untrusted input, not instructions. This is a mitigation, not a guarantee: instructions written as plain visible text cannot be filtered out. Don't grant this action to a workflow with secrets or permissions beyond what a successful "the agent did whatever the issue text said" outcome would be acceptable for.
- **Run logs are never posted back to the issue/PR.** The failure comment links to the Actions run instead of quoting log output. Agent output can contain anything the model was coaxed into printing (including environment values), and GitHub's secret masking only applies to the Actions log viewer -- not to text re-posted through the API -- so copying logs into a public comment would be an exfiltration channel. Logs stay on the run page, which follows your repository's access controls.
- **Installers are fetched over TLS but not checksum-verified.** The action installs cruise via its release installer script and Claude Code via `claude.ai/install.sh` (`curl | sh`). This is the same trust model as rustup et al., but it does mean a compromise of either download endpoint would run attacker code with the job's secrets. Pin `cruise_version` to a tag if you want to at least avoid silently tracking `latest`.
- **PRs opened via `GITHUB_TOKEN` don't trigger other workflows.** This is a deliberate GitHub Actions anti-recursion rule, and it only applies to the plain `GITHUB_TOKEN` fallback path -- PRs/pushes made with the cruise-agent App's installation token (the default, when the App is installed) trigger `on: pull_request` workflows normally, since GitHub treats App-authenticated actions as coming from a distinct actor. If you're on the `GITHUB_TOKEN` fallback and need CI to run on cruise's PRs, either install the App, use your own PAT/GitHub App token via `github_token`, or add a step that closes/reopens the PR.
- **Cross-fork PRs.** In PR mode, cruise pushes updates using the branch tracking `gh pr checkout` sets up. For PRs from forks, this only succeeds if the token has write access to the fork (uncommon for `GITHUB_TOKEN`); otherwise the push fails and is reported as a failure rather than silently pushing to the wrong branch.
- **Runner isolation.** Each run points `XDG_DATA_HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` at `$RUNNER_TEMP`, so cruise's session/worktree state never leaks between jobs and never touches a persistent runner's home directory.

## Troubleshooting

- **Nothing happens after mentioning `@cruise`.** Check the workflow run list for a skipped/no-op run: the gate step logs why it declined (event type, action, missing trigger phrase, or insufficient actor permission).
- **"actor has insufficient permission".** The commenter needs `write`, `maintain`, or `admin` access to the repository.
- **`gh pr checkout` / push fails in PR mode.** Usually a cross-fork PR (see above) or the workflow is missing `permissions: contents: write`.
- **"cruise completed but no pull request was created".** cruise ran (and may have pushed a branch), but `gh pr create` failed. Check that the workflow grants `permissions: pull-requests: write` and that branch protection / repository rules allow creating PRs from the pushed branch.
- **Run always falls back to `GITHUB_TOKEN` (`used_app` output is `false`).** Check, in order: the `cruise-agent` App is installed on this repository ([install link](https://github.com/apps/cruise-agent/installations/new)); the workflow grants `permissions: id-token: write`; `token_exchange_url` is not empty and reachable. The `token` step's log line explains which of these failed.
- **Self-hosted runners** need `git`, `curl`, `jq`, `python3`, and the `gh` CLI on `PATH` (all preinstalled on GitHub-hosted runners).
