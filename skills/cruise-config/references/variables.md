# Variable reference

Variables can be referenced as `{name}` inside `prompt` (including content loaded by
`prompt_file`) / `command` / `env` / `plan` / `instruction` / `when.exists`.
Availability depends on the execution phase and step type. Referencing an undefined
variable is an error.

## Variable list

| Variable | Description |
|----------|-------------|
| `{input}` | Initial input from the CLI argument or stdin; when empty, the first prompt step with an `instruction:` asks the user interactively and stores the entry here |
| `{prev.output}` | LLM output of the previous step |
| `{prev.input}` | User text input from the previous option step |
| `{prev.stderr}` | Stderr captured from the previous command step |
| `{prev.success}` | Exit status of the previous command step (`"true"` / `"false"` string) |
| `{plan}` | Absolute path of the session's plan file (set automatically by `cruise run`) |
| `{plan.language}` | Effective language used for built-in planning prompts (from `CRUISE_LANGUAGE_PLAN`, `languages.plan`, the legacy field, locale inference, or the default); available while resolving planning prompts only |
| `{pr.number}` | PR number, available after a PR has been created |
| `{pr.url}` | PR URL, available after a PR has been created |
| `{pr.language}` | Effective language used for PR title/body generation (from `CRUISE_LANGUAGE_PR`, `languages.pr`, the legacy field, locale inference, or the default) |

## Parser behavior

The substitution is done by a hand-written parser, with Rust-`format!`-style brace escaping. Keep these behaviors in mind:

- Variable names are the characters between `{` and `}`.
- Literal braces are escaped like Rust's `format!`: `{{` → `{`, `}}` → `}`. E.g. `"{{input}}"` → the literal string `"{input}"` (not a lookup of `input`).
- An unclosed `{` is an error (`InvalidTemplateSyntax`), not emitted literally.
- A lone `}` (not part of `}}`) is also an error (`InvalidTemplateSyntax`).
- `{}` (empty variable name) is an error (`EmptyVariableReference`).
- Referencing an undefined variable returns `UndefinedVariable`.

## Availability

- `{plan}` is set automatically by `cruise run` to the session's `plan.md` absolute path. It is undefined outside `cruise run`.
- `{plan.language}` is registered for planning-phase prompts, but normal workflow execution via `cruise run` initializes only `{plan}` (plus runtime `{prev.*}` values), so references to `{plan.language}` in execution-step fields fail with `UndefinedVariable`.
- `{pr.number}` / `{pr.url}` are defined only after `gh pr create` succeeds — effectively only inside `after-pr`.
- After a successful prompt step, `{prev.output}` and `{prev.stderr}` are set, `{prev.input}` is cleared, and `{prev.success}` is retained.
- After a completed command step, `{prev.stderr}` and `{prev.success}` are set, while `{prev.output}` and `{prev.input}` are cleared.
- After a non-empty option step, `{prev.output}` is cleared, `{prev.input}` is updated only for a `text-input` choice (a selector leaves the previous value), and `{prev.stderr}` / `{prev.success}` are retained. Skipped steps and empty-option steps leave all `{prev.*}` unchanged.

## `{model}` is not a variable

`{model}` is a special placeholder resolved only inside the top-level `command` array. It cannot be used inside `prompt` / `prompt_file` / `instruction` / `command` step fields (see [top-level.md](top-level.md)). The same brace-escaping rules apply there: `{{model}}` is the literal string `{model}`, and any other unescaped `{name}` (as well as malformed brace syntax) is an error.
