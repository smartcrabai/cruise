Verify that the implemented behavior follows the exact user-approved Quint model provided below. Do not change the model, production code, tests, or task scope in this step. If behavior does not match, list the blockers. Do not rewrite results to pass.

Persist the exact model bytes in a new or task-owned `.qnt` file in the repository. Inspect existing CI and connect the model's pinned Quint 0.33.0 `typecheck` and `test` to the repository's normal PR checks. If none exists, add the smallest check for the repository's actual provider. Do not overwrite unrelated specifications or CI. Check the relevant existing real tests and actual target state. Reuse observed test-first/implementation, refactor baseline, or E2E results when they apply; do not rerun full suites. Report exact commands and real results, never invented data, numbers without units, or an unverified URL. Separate commands that you observed from earlier step reports. Do not include secrets, private identifiers, absolute paths, tokens, environment data, or recordings unavailable to the PR audience. Use public/synthetic saved values only.

Return only one JSON object (no fences):

```
{{"model":"relative/path/to/model.qnt","ci":"relative/path/to/ci-config","blockers":[],"checks":[{{"command":"exact command","exit":0,"excerpt":"one short attributable stdout/stderr line, or empty","earlier":false}}],"measurements":[{{"label":"public measurement and unit","before":12,"after":18}}],"recordings":["https://forge.example/owner/repo/.../public-recording"]}}
```

All lists may be empty when unavailable, except blockers must name real mismatches or missing required setup. `before` may be null for a saved value with no comparable baseline. Set `earlier` true for an earlier step's report, including expected TDD red results, not a command executed here. The host independently runs and records Quint commands.
