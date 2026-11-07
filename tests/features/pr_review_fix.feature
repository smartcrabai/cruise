@pr_review_fix
Feature: Classify completion of the all-reviewer fix loop
  The real after-PR review loop distinguishes success, stalled work, and exhausted iterations.

  Scenario: No actionable threads completes without invoking the prompt
    Given the all-reviewer fixture has "empty" threads and a limit of 1
    When the existing PR workflow runs
    Then the review loop succeeds without a prompt

  Scenario: Unchanged threads fail as no progress even at the iteration limit
    Given the all-reviewer fixture has "unchanged" threads and a limit of 1
    When the existing PR workflow runs
    Then the review loop fails for "no progress"
    And the prompt ran in a clean git repository

  Scenario: Some progress with remaining threads fails at the iteration limit
    Given the all-reviewer fixture has "partial" threads and a limit of 1
    When the existing PR workflow runs
    Then the review loop fails for "unresolved review threads remain after 1 iterations"
    And the prompt ran in a clean git repository

  Scenario: All actionable threads disappearing completes successfully
    Given the all-reviewer fixture has "complete" threads and a limit of 1
    When the existing PR workflow runs
    Then the review loop succeeds after the prompt
    And the prompt ran in a clean git repository

  Scenario: A read-only review thread is excluded without invoking the prompt
    Given the all-reviewer fixture has "excluded" threads and a limit of 1
    When the existing PR workflow runs
    Then the review loop succeeds without a prompt

  Scenario: Progress permits a second iteration that completes the loop
    Given the all-reviewer fixture has "two iterations" threads and a limit of 2
    When the existing PR workflow runs
    Then the review loop succeeds after the prompt
    And the prompt ran 2 times in a clean git repository
