The user has requested changes to the implementation plan. The current plan is stored at {plan}.

Write the updated plan in {plan.language}.

Requested changes:
---
{prev.input}

**Procedure:**
1. Read the current plan at {plan} so you are working from its exact, up-to-date text.
2. If the requested change is ambiguous, call the `ask_user` tool to clarify before editing.
3. Apply the changes:
   - For targeted edits, call `update_plan` with an exact `old` snippet from the current plan and its `new` replacement. If `old` does not match, re-read the plan and retry with a verbatim snippet.
   - For sweeping rewrites, call `submit_plan` with the complete revised plan.

Make all requested modifications, then ensure the plan on disk reflects them.
