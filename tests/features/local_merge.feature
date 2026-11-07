@local_merge
Feature: Local squash merge lifecycle
  A completed session can be integrated without creating a pull request.

  Background:
    Given a completed local worktree session without a pull request

  Scenario: Stage committed and uncommitted work without cleanup
    Given the session has committed and uncommitted changes
    When I stage the local merge
    Then both changes are staged without a base commit and the session is retained

  Scenario: Refuse a base checked out on the wrong branch
    Given the session has committed and uncommitted changes
    And the base is checked out on a different branch
    When I merge locally and clean up
    Then the wrong branch is reported and both checkouts are retained unchanged

  Scenario: Preserve an untracked base file that would be overwritten
    Given the session has committed and uncommitted changes
    And an untracked base file would be overwritten by the merge
    When I stage the local merge
    Then the untracked base file and session are retained

  Scenario: Merge and clean up committed and uncommitted work
    Given the session has committed and uncommitted changes
    When I merge locally and clean up
    Then one squash commit contains both changes and the session is cleaned up

  Scenario: A dirty base is refused without changing either checkout
    Given the session has committed and uncommitted changes
    And the base has a tracked modification
    When I merge locally and clean up
    Then the dirty base and session are unchanged

  Scenario: A conflict restores the base and retains the session
    Given the base and session have conflicting commits
    When I merge locally and clean up
    Then the conflict is reported and the base is restored with the session retained

  Scenario: Cleanup can be retried without another squash commit
    Given the session has committed and uncommitted changes
    And the session worktree is locked against removal
    When I merge locally and clean up
    Then the merge commit is recorded and cleanup fails
    When I unlock the session worktree and retry cleanup
    Then cleanup succeeds without another commit
