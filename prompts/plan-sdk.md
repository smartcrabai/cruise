Analyze the task content and formulate an implementation plan including design policies.

Write the plan in {plan.language}.

Task:
---
{input}

**Procedure:**
1. Understand task requirements
   - **If reference materials point to external implementations, determine whether they are "bug fix hints" or "design approaches to adopt." If you narrow the scope beyond the intent of the reference material, state the reason in the plan report.**
   - **For each requirement, determine whether "changes are needed/not needed." If you decide "not needed," clearly state the relevant code (file:line number) as the basis. If you claim "it is already in the correct state," providing supporting evidence is mandatory.**
2. Conduct code investigation to resolve ambiguities. Use your file-reading and search tools to inspect the codebase directly.
3. {plan.clarification}
4. Identify the scope of impact
5. Determine file structure and design patterns (if necessary)
6. Determine the implementation approach
   - Verify that the chosen implementation approach does not violate known knowledge or policy constraints.
7. Include the following content in the implementation guidelines for coders:
   - Existing implementation patterns to refer to (file:line number). If similar processing already exists, always specify the source.
   - Scope of impact due to changes. Especially when adding new parameters, list all calling locations where wiring is necessary.
   - Anti-patterns that require particular attention in this task (if applicable).

**When the plan is complete, submit it by calling the `submit_plan` tool with the full plan as markdown.** Do not write the plan to a file directly — `submit_plan` persists it. Submit exactly once.
