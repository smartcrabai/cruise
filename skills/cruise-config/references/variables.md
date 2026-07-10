# Variable reference

Variables can be referenced as `{name}` inside `prompt` / `command` / `env` / `plan` / `instruction` / `when.exists`. Referencing an undefined variable is an error.

## Variable list

| Variable | Description |
|----------|-------------|
| `{input}` | Initial input from the CLI argument or stdin; when empty, the first prompt step with an `instruction:` asks the user interactively and stores the entry here |
| `{prev.output}` | LLM output of the previous step |
| `{prev.input}` | User text input from the previous option step |
| `{prev.stderr}` | Stderr captured from the previous command step |
| `{prev.success}` | Exit status of the previous command step (`"true"` / `"false"` string) |
| `{plan}` | Absolute path of the session's plan file (set automatically by `cruise run`) |
| `{pr.number}` | PR number, available after a PR has been created |
| `{pr.url}` | PR URL, available after a PR has been created |

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
- `{pr.number}` / `{pr.url}` are defined only after `gh pr create` succeeds — effectively only inside `after-pr`.
- `{prev.*}` availability depends on the previous step's type:
  - After a prompt step: `{prev.output}` only.
  - After an option step: `{prev.input}` only (set when a `text-input` option was chosen).
  - After a command step: `{prev.stderr}` and `{prev.success}`.

## `{model}` is not a variable

`{model}` is a special placeholder resolved only inside the top-level `command` array. It cannot be used inside `prompt` / `instruction` / `command` step fields (see [top-level.md](top-level.md)). The same brace-escaping rules apply there: `{{model}}` is the literal string `{model}`, and any other unescaped `{name}` (as well as malformed brace syntax) is an error.
