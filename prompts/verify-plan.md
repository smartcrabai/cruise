Derive formal specifications from the agreed requirements in this plan:

{plan}

Write state machines and invariants in Quint and enumerative constraints in Alloy. Output specifications in fenced blocks with the exact info string `quint` or `alloy`. Do not import external files or change any files. Any explanatory text must be in {plan.language}.

For Quint, define one main module with `action init` and `action step`, invariant definitions named `inv*` and temporal properties named `prop*`. Bound integers to finite domains. TLC explores the state space and ignores `--max-steps`, so ensure the state space is finite.

For Alloy, give each `run` or `check` command a distinct explicit name and an explicit `for` scope. Include at least one `run` that has a satisfying witness. Model only requirements supported by the plan and do not claim verification has already occurred.
