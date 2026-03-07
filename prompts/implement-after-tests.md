Implement according to the plan for {plan} such that the tests pass.

**Important**: Tests have already been created. Implement such that the existing tests pass.
- Review existing test files to understand the expected behavior
- Implement production code to make the tests pass
- While tests have been created and generally do not need to be added, you may add them if necessary
- You may modify tests if correction is required
- Build confirmation is mandatory. After implementation, run the build (type check) and ensure there are no type errors
- Test execution is mandatory. After a successful build, always run tests and ensure all tests pass
- If new contract strings such as file names or configuration key names are introduced, define them as constants in a single location

**Self-check before implementation completion (mandatory):**
Before running the build and tests, please check the following:
- If new parameters/fields were added, confirmed via grep that they are actually passed from the caller
- Confirmed whether fallback is truly necessary in places using `??`, `||`, `= defaultValue`
- Confirmed that refactored code/exports that were replaced do not remain
- Confirmed that features not in the task instructions have not been added
- Confirmed that if/else statements do not call the same function with only differences in arguments
- Confirmed that new code is consistent with existing implementation patterns (API call method, type definition method, etc.)

**Required Output (include headings)**
## Work Summary
- {Summary of work performed}
## Changes Made
- {Summary of changes made}
## Build Results
- {Build execution results}
## Test Results
- {Test execution command and results}
