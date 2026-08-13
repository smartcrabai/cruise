---
name: code-docs-skills-consistency
description: Verify docs/, examples/, and skills/ still match the code they describe — input names, verbatim error strings, cross-references orphaned by a rewrite
origin: session 2026-08-13
---

# Code ↔ documentation / skills consistency

## When to apply

Diffs touching a user-facing contract or the prose describing one:
`action.yml` inputs, user-facing message strings in `action/scripts/*.sh`,
`src/cli.rs`/`src/config.rs` flags and config keys, `docs/`, `examples/`,
`skills/` (`cruise-cli`, `cruise-config` incl. `references/` and
`examples/`, `cruise-plan`), `README.md`.

Skip for internal Rust changes with no change to a flag, config key, action
input, emitted message, or documented default.

## What to check

- Every new/renamed `action.yml` input appears in the `docs/` input table,
  and its `description:` doesn't contradict the table or prose.
- Error strings quoted in docs match what the script emits **verbatim** —
  diff against the `hard_fail`/`::error::` line, don't eyeball. When a gate
  condition gains a term, the quoted message must too.
- Changed flags, subcommands, or YAML keys are reflected in `skills/`
  (including `references/*.md` and the `examples/*.yaml` samples). Nothing
  compiles the skills, so they go stale silently.
- Behavioral claims still hold: validation promises, resolution order,
  defaults, "at least one of X/Y is required".
- When an example is rewritten to demonstrate a *different* mechanism, grep
  the repo for links to it and re-attribute every sentence that introduced
  it as the example for the old one — **including sentences the diff never
  touched**. If the last example of a still-supported feature is gone, stop
  promising one.
- Read the diff's **removal** side: an unrelated pre-existing line deleted
  while rewriting its neighbour is a content regression. Compare against
  `git show HEAD:<file>` when a hunk removes more than it replaces.
- Markdown list items keep consistent indentation (no stray leading space).
- Examples run as written: embedded JSON parses, identifiers match across
  inputs (a provider id in `providers` ↔ `provider_api_keys` ↔
  `model`/`plan_model`), `actionlint` passes.
