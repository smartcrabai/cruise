# Contributing

Thank you for your interest in contributing.

## How to Contribute

1. Open an issue or discussion for non-trivial changes before starting work.
2. Fork the repository and create a topic branch.
3. Keep changes focused and include tests or documentation updates when appropriate.
4. Run the repository's formatter, linter, and test commands before opening a pull request.
5. Open a pull request with a clear summary and any relevant context.

## Prompt evaluation

The prompt evaluation harness checks prompt quality against fixed cases. Ordinary
`cargo test --all-features` runs deterministic contracts without calling a model.
Live evaluation is ignored by default, requires explicit opt-in, and can incur
provider charges. It is a measurement tool, not an automatic prompt editor.

For a local run, install and authenticate the relevant CLI and select distinct
candidate and judge models. For example, with the default jcode SDK:

```sh
export CRUISE_PROMPT_EVAL=1
export CRUISE_PROMPT_EVAL_MODEL='provider/candidate-model'
export CRUISE_PROMPT_EVAL_JUDGE_MODEL='provider/judge-model'
cargo test --all-features --test prompt_eval -- --ignored --nocapture
```

`CRUISE_PROMPT_EVAL_SDK` defaults to `jcode`; `claude` and `opencode` are
also supported when their CLIs and credentials are configured locally. Model
syntax depends on the selected SDK. Optional settings are:

- `CRUISE_PROMPT_EVAL_BASE`: git ref to compare against the working tree.
  The ref is resolved to a commit and the base prompt templates are loaded
  from that commit, with the same cases and judge used for both sides.
- `CRUISE_PROMPT_EVAL_CASE`: regex selecting row IDs.
- `CRUISE_PROMPT_EVAL_REPEAT`: trials per row, default 3, unless a case
  specifies its own repeat count.
- `CRUISE_PROMPT_EVAL_CONCURRENCY`: concurrent trials, default 3.
- `CRUISE_PROMPT_EVAL_TRIAL_TIMEOUT`: whole-trial timeout in seconds, including candidate execution and judging, default 900. Each canary judge invocation has the same timeout independently.
- `CRUISE_PROMPT_EVAL_OUTPUT`: output root, default `target/prompt-eval/`.

The harness runs deterministic checks before rubric grading and calibrates the
judge with canaries. A row passes only if every trial passes. A comparison
reports regressions and improvements, but a failed or errored candidate still
fails the run. Expect roughly rows × repeats × two model calls, doubled for a
base comparison, plus canary grading calls. Actual calls may be lower when a
deterministic check fails and skips grading.

Results under `target/prompt-eval/` include a manifest, canary results,
per-trial JSONL and prompt/output/judge files, and a Markdown summary. These
artifacts may contain prompts, model output, or fixture data, so review them
before sharing. They are build artifacts and should not be committed.

For a GitHub Actions run, configure a `prompt-eval` environment with required
reviewers and the provider secrets needed by the selected models
(`ANTHROPIC_API_KEY` and/or `OPENAI_API_KEY`). In Actions, manually run
**Prompt evaluation** with distinct candidate and judge models. Optionally set
base, repeat, and case. The workflow uses jcode only, fetches full history for
base refs, and uploads `prompt-eval-reports` even after failures when reports
exist. Artifact retention is 14 days. Do not put credentials in workflow
inputs, case files, or reports.

## Pull Request Guidelines

- Keep pull requests small and reviewable.
- Explain why the change is needed, not only what changed.
- Link related issues when applicable.
- Be respectful and constructive in reviews and discussions.

## Security Issues

Please do not disclose security vulnerabilities in public issues. See `SECURITY.md` for reporting instructions.
