@pr_review_fix
Feature: Classify completion of the all-reviewer fix loop
  The real after-PR review loop distinguishes success, stalled work, and exhausted iterations.

  Scenario: No actionable threads completes without invoking the prompt
    Given the all-reviewer fixture has "empty" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop succeeds without a prompt

  Scenario: Unchanged threads fail as no progress even at the iteration limit
    Given the all-reviewer fixture has "unchanged" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "no progress"
    And the prompt ran in a clean git repository

  Scenario: Some progress with remaining threads fails at the iteration limit
    Given the all-reviewer fixture has "partial" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "unresolved review threads remain after 1 iterations"
    And the prompt ran in a clean git repository

  Scenario: All actionable threads disappearing completes successfully
    Given the all-reviewer fixture has "complete" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop succeeds after the prompt
    And the prompt ran in a clean git repository

  Scenario: A read-only review thread is excluded without invoking the prompt
    Given the all-reviewer fixture has "excluded" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop succeeds without a prompt

  Scenario: Progress permits a second iteration that completes the loop
    Given the all-reviewer fixture has "two iterations" threads and a limit of 2
    When the production after-PR review handler runs
    Then the review loop succeeds after the prompt
    And the prompt ran 2 times in a clean git repository

  Scenario: Cruise commits validated edits, pushes the PR ref, then applies replies
    Given the all-reviewer fixture has "host-commit" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop succeeds after the prompt
    And the prompt ran in a clean git repository
    And the host committed and pushed only the validated prompt edits without hooks

  Scenario: A concurrent PR head change defeats publication before review writes
    Given the all-reviewer fixture has "remote-advanced" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "git push to the pull-request head failed"
    And the host did not overwrite the advanced PR head

  Scenario: Host consent with no edits creates no commit
    Given the all-reviewer fixture has "host-no-changes" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop succeeds after the prompt
    And the host completed a no-change turn without a commit

  Scenario: A dirty worktree refuses the edit turn before prompting
    Given the all-reviewer fixture has "dirty-baseline" threads and a limit of 1
    When the production after-PR review handler runs
    Then the host refused the edit turn before running the prompt

  Scenario: Detached HEAD refuses the edit turn before prompting
    Given the all-reviewer fixture has "detached-head" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "HEAD must be attached"
    And the host refused the edit turn before running the prompt

  Scenario: A different source branch is refused before prompting
    Given the all-reviewer fixture has "wrong-branch" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "does not match the pull request head ref"
    And the host refused the edit turn before running the prompt

  Scenario: A mismatched upstream is refused before prompting
    Given the all-reviewer fixture has "wrong-tracking" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "does not track the pull request head ref"
    And the host refused the edit turn before running the prompt

  Scenario: A local HEAD that differs from the authoritative PR head is refused
    Given the all-reviewer fixture has "wrong-head" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "does not match the pull request's current head"
    And the host refused the edit turn before running the prompt

  Scenario: Invalid actions leave prompt edits uncommitted
    Given the all-reviewer fixture has "invalid-response" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "invalid review action response"
    And the prompt edits remain uncommitted

  Scenario: Ineligible actions leave prompt edits uncommitted
    Given the all-reviewer fixture has "ineligible-response" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "no permission to reply"
    And the prompt edits remain uncommitted

  Scenario: Commit consent is not passed to the prompt or inferred for custom steps
    Given the all-reviewer fixture has "custom-edit" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "uncommitted changes"
    And the prompt edits remain uncommitted

  Scenario: Configured Git filters are refused before execution
    Given the all-reviewer fixture has "external-filter" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "unsafe Git configuration"
    And the host refused the edit turn before running the prompt
    And the configured filter was not executed

  Scenario: An in-progress merge refuses the edit turn
    Given the all-reviewer fixture has "in-progress-merge" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "in-progress Git operation"
    And the host refused the edit turn before running the prompt

  Scenario: A live permission revocation prevents host publication
    Given the all-reviewer fixture has "permission-revoked" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "no permission to reply"
    And the prompt edits remain uncommitted

  Scenario: Reply-only review is withheld if the PR head changes
    Given the all-reviewer fixture has "reply-race" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "pull request identity or head changed"
    And the reply-only turn did not write after the PR head changed

  Scenario: The whole reply batch is revalidated before the first write
    Given the all-reviewer fixture has "write-race" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "thread 't2' no longer exists"
    And the host published edits but withheld the incomplete review batch

  Scenario: A concurrent local branch update defeats the compare-and-swap
    Given the all-reviewer fixture has "commit-cas-race" threads and a limit of 1
    When the production after-PR review handler runs
    Then the review loop fails for "branch moved before the host commit could be attached"
    And the concurrent local branch update was never published
