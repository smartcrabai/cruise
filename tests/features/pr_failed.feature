Feature: Recovering from pull request publication failure
  Workflow execution and pull request publication are separate operations.

  Scenario: A publication retry with no commits completes without rerunning the workflow
    Given a PR Failed session with an unchanged worktree
    When the user creates the PR
    Then the session is completed with the no-commits reason
    And the workflow was not rerun
    And the worktree still exists

  Scenario: PR Failed actions do not offer workflow retry
    Given a PR Failed session with an unchanged worktree
    Then the available actions include Create PR and Reset to Planned but not Retry

  Scenario: Create PR cannot be used for a planned session
    Given a planned session with an unchanged worktree
    When the user creates the PR
    Then the operation is rejected and the planned session is unchanged
    And the workflow was not rerun

  Scenario: A missing worktree is rejected without losing the failure
    Given a PR Failed session with an unchanged worktree
    And the session has no worktree
    When the user creates the PR
    Then the operation is rejected and the PR failure is preserved
    And the workflow was not rerun

  Scenario: A Git failure during publication preserves the retryable session
    Given a PR Failed session with an unchanged worktree
    And the base repository is unavailable to Git
    When the user creates the PR
    Then the publication fails and the PR Failed state is saved
    And the workflow was not rerun
    And the worktree still exists
