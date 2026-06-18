Interview the user relentlessly about this task until you both reach a shared understanding, then write the implementation plan.

Conduct the interview and write the final plan in {plan.language}.

Task:
---
{input}

**Interview procedure:**
1. Walk down each branch of the design tree, resolving dependencies between decisions one-by-one until the plan is fully pinned down.
2. Ask the questions **one at a time** by calling the `ask_user` tool. Do not batch multiple open questions into a single call.
3. For every question, **provide your own recommended answer** along with a short rationale so the user can simply confirm or redirect.
4. If a question can be answered by exploring the codebase, **explore the codebase instead of asking.** Use your file-reading and search tools to inspect the code directly, and only ask the user about genuine product/design ambiguities that the code cannot resolve.
5. Keep grilling until there are no unresolved branches: scope, edge cases, error handling, data flow, impacted call sites, and the chosen implementation approach are all settled.

**While interviewing, ground your questions in the actual code:**
- For each requirement, determine whether "changes are needed/not needed." If you decide "not needed," cite the relevant code (file:line) as the basis.
- If reference materials point to external implementations, determine whether they are "bug fix hints" or "design approaches to adopt," and confirm the interpretation with the user when it changes the plan.

**When — and only when — the interview has resolved every branch, write the plan and submit it by calling the `submit_plan` tool with the full plan as markdown.** Do not write the plan to a file directly — `submit_plan` persists it. Submit exactly once, after the interview is complete. The submitted plan must include, for the coders:
- The agreed-upon implementation approach and design policies (reflecting the decisions reached during the interview).
- Existing implementation patterns to refer to (file:line). If similar processing already exists, always specify the source.
- Scope of impact due to changes. Especially when adding new parameters, list all call sites where wiring is necessary.
- Anti-patterns that require particular attention in this task (if applicable).
