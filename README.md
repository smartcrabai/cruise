# cruise

A CLI tool that orchestrates coding agent workflows defined in a YAML config file.

Cruise wraps CLI coding agents such as `claude -p` and drives them through a declarative workflow: plan → approve → write tests → implement → test → review → open PR. It handles variable passing between steps, conditional branching, and loop control.

## Installation

### cargo install

```sh
cargo install cruise
```

### Homebrew

```sh
brew install takumi3488/tap/cruise
```

## Usage

```sh
# Run the default cruise.yaml workflow
cruise "implement the feature"

# Use a custom config file
cruise -c workflow.yaml "task"

# Resume from a specific step
cruise --from implement "implement the feature"

# Preview the flow without executing
cruise --dry-run "implement the feature"
```

### CLI Reference

```
cruise [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Initial input passed to the workflow (reads from stdin if omitted and stdin is piped)

Options:
  -c, --config <PATH>          Path to the workflow config file [default: cruise.yaml]
  --from <STEP>                Step name to start from (resume mid-workflow)
  --max-retries <N>            Maximum times a loop edge may be traversed [default: 10]
  --rate-limit-retries <N>     Maximum rate-limit retries per step [default: 5]
  --dry-run                    Print the workflow flow without executing
```

## Config File Reference

### Basic Structure

```yaml
command:
  - claude
  - --model
  - "{model}"
  - -p

model: sonnet             # default model for all prompt steps (optional)

plan: plan.md             # optional: file path bound to the {plan} variable

steps:
  step_name:
    # step configuration
```

### Dynamic Model Selection

When the `command` array contains a `{model}` placeholder, cruise resolves it at runtime based on the effective model for each step:

- **Model specified** (via top-level `model` or step-level `model`): replaces `{model}` with the model name.
- **No model specified**: removes the `{model}` argument and its immediately-preceding `--model` flag automatically.

A step-level `model` field overrides the top-level `model` default for that step only.

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

### Step Types

#### Prompt Step (LLM call)

```yaml
steps:
  planning:
    model: claude-opus-4-5        # model to use (optional; overrides top-level model)
    instruction: |                # system prompt (optional)
      You are a senior engineer.
    prompt: |                     # prompt body (required)
      Create an implementation plan for:
      {input}
    output: plan                  # variable to store the output in (optional)
                                  # if output matches the top-level plan field name,
                                  # the result is also written to that file automatically
```

#### Command Step (shell execution)

```yaml
steps:
  run_tests:
    command: cargo test           # single command (required)
    description: Running tests    # display label (optional)

  lint_and_test:
    command:                      # list of commands: run sequentially, stop on first failure
      - cargo fmt --all
      - cargo clippy -D warnings
      - cargo test
```

#### Option Step (interactive selection)

Each item in `option` is either a `selector` (menu choice) or a `text-input` (free-text prompt):

```yaml
steps:
  review_plan:
    description: Review the plan
    option:
      - selector: Approve and continue   # shown in selection menu
        next: implement
      - selector: Revise the plan
        next: planning
      - text-input: Other (free text)    # shows a text prompt when selected;
        next: planning                   # entered text is available as {prev.input}
      - selector: Cancel
        next: ~                          # null next = end of workflow
```

### Flow Control

#### Explicit next step

```yaml
steps:
  step_a:
    command: echo "hello"
    next: step_c                  # jump over step_b
  step_b:
    command: echo "skipped"
  step_c:
    command: echo "world"
```

#### Skipping a step

```yaml
steps:
  optional_step:
    command: cargo fmt
    skip: true                    # always skip

  fix_errors:
    command: cargo fix
    skip: prev.success            # skip if the variable "prev.success" resolves to "true"
```

The `skip` field accepts a static boolean (`true`/`false`) or a variable reference string. When a variable reference is given, the step is skipped if that variable's current value is `"true"`.

#### Conditional execution (file-changed detection)

```yaml
steps:
  implement:
    command: claude -p "implement: {input}"

  run_tests:
    command: cargo test

  commit:
    command: git commit -am "feat: {input}"
    if:
      file-changed: implement     # only run if files changed since `implement`
```

> **Note:** Snapshots for `file-changed` checks are taken **only after command steps**. Prompt and option steps do not create snapshots. If the referenced step has never run (no snapshot exists), the condition evaluates to `false` and the step is skipped.

### Variable Reference

| Variable | Description |
|----------|-------------|
| `{input}` | Initial input from CLI argument or stdin |
| `{prev.output}` | LLM output from the previous step |
| `{prev.input}` | User text input from the previous option step |
| `{prev.stderr}` | Stderr captured from the previous command step |
| `{prev.success}` | Exit status of the previous command step (`true`/`false`) |
| `{plan}` | Contents of the file specified by the top-level `plan` field; also written automatically when a prompt step uses `output: plan` |
| `{name}` | Named variable defined via the `output` field |

> **Note:** `{model}` is **not** a template variable — it is a special placeholder resolved only within the top-level `command` array. It is not available inside `prompt`, `instruction`, or `command` step fields.

## Example Config

### Full Development Flow

```yaml
command:
  - claude
  - --model
  - "{model}"
  - -p

model: sonnet

plan: .cruise/plan.md

steps:
  plan:
    model: opus
    instruction: "What will you do?"
    prompt: |
      I am trying to implement the following features. Create an implementation plan.
      ---
      {input}
    output: plan

  approve-plan:
    description: "{prev.output}"
    option:
      - selector: Approve
        next: write-tests
      - text-input: Fix
        next: fix-plan
      - text-input: Ask
        next: ask-plan

  fix-plan:
    model: opus
    prompt: |
      The user has requested the following changes to the {plan} implementation plan. Make the modifications:
      {prev.input}
    next: approve-plan

  ask-plan:
    prompt: |
      The user has the following questions about the implementation plan for {plan}. Provide answers:
      {prev.input}
    next: approve-plan

  write-tests:
    prompt: |
      Based on the {plan} implementation schedule, please first create the test code,
      then update the {plan} if necessary.

  implement:
    prompt: |
      Tests have been created according to {plan}. Please implement them to pass.
      If necessary, update {plan}.

  test:
    command:
      - cargo fmt --all
      - cargo clippy --fix --allow-dirty -D warnings
      - cargo test

  fix-test-error:
    skip: prev.success            # skip if tests passed
    prompt: |
      The following error occurred. Please correct it:
      ---
      {prev.stderr}
    next: test

  pr:
    prompt: create a PR
    if:
      file-changed: test          # only if files changed since `test` step
```

### Simple Auto-Commit Flow

```yaml
command:
  - claude
  - -p

steps:
  implement:
    prompt: "{input}"

  commit:
    command: git add -A && git commit -m "feat: {input}"
    if:
      file-changed: implement
```

## Rate Limit Retry

When a rate-limit error (HTTP 429) is detected in a prompt or command step, cruise retries with exponential backoff:

- Initial delay: 2 seconds
- Maximum delay: 60 seconds
- Default retry count: 5 (override with `--rate-limit-retries`)

## License

MIT
