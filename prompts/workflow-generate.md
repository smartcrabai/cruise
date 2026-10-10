You write a cruise workflow configuration file.

Reply with the YAML document only. Do not wrap it in a Markdown code fence and do not add any explanation before or after it. Your reply is parsed as raw YAML exactly as written.

## Requirements

Create a workflow that satisfies this description:

{description}

## Rules

- Output only fields that cruise supports, as described by the JSON Schema below. `steps` is required. `command` and `sdk` are mutually exclusive.
- Unknown top-level keys are silently ignored by cruise, so a typo there has no effect. Unknown keys inside a step or `retry:` block are rejected. Double-check every field name.
- Control step transitions only with the deterministic fields cruise already interprets: `next`, `if`, `when`, `option`, and groups. Never rely on free-form text to choose the next step.
- Every transition target must name an existing step, and every loop must have a way to reach the end of the workflow.
- Do not create or reference files other than ones you inline in the YAML.
- A passing validation does not prove that the commands in the workflow are safe or that the workflow succeeds when run.

## JSON Schema

```json
{schema}
```
