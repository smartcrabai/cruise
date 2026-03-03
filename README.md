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
  - -p

plan: plan.md  # optional: file path bound to the {plan} variable

steps:
  step_name:
    # step configuration
```

### Step Types

#### Prompt Step (LLM call)

```yaml
steps:
  planning:
    model: claude-opus-4-5        # model to use (optional)
    instruction: |                # system prompt (optional)
      You are a senior engineer.
    prompt: |                     # prompt body (required)
      Create an implementation plan for:
      {input}
    output: plan                  # variable to store the output in (optional)
```

#### Command Step (shell execution)

```yaml
steps:
  run_tests:
    command: cargo test           # shell command to run (required)
    description: Running tests    # display label (optional)
```

#### Option Step (interactive selection)

```yaml
steps:
  review_plan:
    description: Review the plan
    option:
      - label: Approve and continue
        next: implement           # step to go to when selected
      - label: Revise the plan
        next: planning
      - label: Cancel
        next: null
    text-input:
      label: Other (free text)
      next: planning
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
```

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

### Variable Reference

| Variable | Description |
|----------|-------------|
| `{input}` | Initial input from CLI argument or stdin |
| `{prev.output}` | LLM output from the previous step |
| `{prev.input}` | User text input from the previous option step |
| `{prev.stderr}` | Stderr captured from the previous command step |
| `{prev.success}` | Exit status of the previous command step (`true`/`false`) |
| `{plan}` | Contents of the file specified by the top-level `plan` field |
| `{name}` | Named variable defined via the `output` field |

## Example Config

### Full Development Flow

```yaml
command:
  - claude
  - -p

plan: plan.md

steps:
  planning:
    model: claude-opus-4-5
    instruction: |
      You are a senior software engineer.
      Favor a test-driven development approach.
    prompt: |
      Create an implementation plan for the following feature:

      {input}

      Output the plan in Markdown.
    output: plan

  review_plan:
    description: |
      Review the implementation plan:

      {plan}
    option:
      - label: Approve and continue
        next: write_tests
      - label: Revise the plan
        next: planning
      - label: Cancel
        next: null

  write_tests:
    prompt: |
      Write tests based on this plan:

      {plan}

  implement:
    prompt: |
      Implement the feature based on this plan:

      {plan}

      Make all tests pass.

  run_tests:
    command: cargo test

  fix_or_done:
    description: Check test results
    option:
      - label: Tests passed
        next: create_pr
      - label: Needs fixing
        next: implement

  create_pr:
    command: gh pr create --fill
    if:
      file-changed: implement
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
