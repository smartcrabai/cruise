Analyze the task content, formulate an implementation plan including design policies, and write it in the {plan} file.

Task:
---
{input}

**Procedure:**
1. Understand task requirements
   - **If reference materials point to external implementations, determine whether they are "bug fix hints" or "design approaches to adopt." If you narrow the scope beyond the intent of the reference material, state the reason in the plan report.**
   - **For each requirement, determine whether "changes are needed/not needed." If you decide "not needed," clearly state the relevant code (file:line number) as the basis. If you claim "it is already in the correct state," providing supporting evidence is mandatory.**
2. Conduct code investigation to resolve ambiguities
3. Identify the scope of impact
4. Determine file structure and design patterns (if necessary)
5. Determine the implementation approach
   - Verify that the chosen implementation approach does not violate known knowledge or policy constraints.
6. Include the following content in the implementation guidelines for coders:
   - Existing implementation patterns to refer to (file:line number). If similar processing already exists, always specify the source.
   - Scope of impact due to changes. Especially when adding new parameters, list all calling locations where wiring is necessary.
   - Anti-patterns that require particular attention in this task (if applicable).
