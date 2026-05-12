# Variable reference

Variables can be referenced as `{name}` inside `prompt` / `command` / `env` / `plan` / `instruction`. Referencing an undefined variable is an error.

## Variable list

| Variable | Description |
|----------|-------------|
| `{input}` | Initial input from the CLI argument or stdin |
| `{prev.output}` | LLM output of the previous step |
| `{prev.input}` | User text input from the previous option step |
| `{prev.stderr}` | Stderr captured from the previous command step |
| `{prev.success}` | Exit status of the previous command step (`"true"` / `"false"` string) |
| `{plan}` | Absolute path of the session's plan file (set automatically by `cruise run`) |
| `{pr.number}` | PR number, available after a PR has been created |
| `{pr.url}` | PR URL, available after a PR has been created |

## Parser behavior

The substitution is done by a hand-written parser. Keep these behaviors in mind:

- Variable names are the characters between `{` and `}`.
- An unclosed `{` is emitted literally (e.g. `"trailing {"` → `"trailing {"`).
- There is no escape syntax like `{{...}}`. `{{input}}` is parsed as a lookup of a variable named `{input`, which is undefined.
- Referencing an undefined variable returns `UndefinedVariable`.

## Availability

- `{plan}` is set automatically by `cruise run` to the session's `plan.md` absolute path. It is undefined outside `cruise run`.
- `{pr.number}` / `{pr.url}` are defined only after `gh pr create` succeeds — effectively only inside `after-pr`.
- `{prev.*}` availability depends on the previous step's type:
  - After a prompt step: `{prev.output}` only.
  - After an option step: `{prev.input}` only (set when a `text-input` option was chosen).
  - After a command step: `{prev.stderr}` and `{prev.success}`.

## `{model}` is not a variable

`{model}` is a special placeholder resolved only inside the top-level `command` array. It cannot be used inside `prompt` / `instruction` / `command` step fields (see [top-level.md](top-level.md)).
