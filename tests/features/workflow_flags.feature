@workflow_flags
Feature: Select workflow flags before dependent skips and execution
  The execution boundary asks once, preserves saved choices, and never prompts automation.

  Scenario: Select an optional step and persist explicit off values
    Given an isolated planned workflow with docs enabled
    When I run the workflow in a terminal and select e2e instead
    Then only the e2e optional effect runs
    And the saved flags enable e2e and disable docs

  Scenario: Confirm an empty selection to turn every declared flag off
    Given an isolated planned workflow with docs enabled
    When I run the workflow in a terminal and clear all flags
    Then no optional effects run
    And both saved declared flags are explicitly off

  Scenario: Enter retains saved defaults
    Given an isolated planned workflow with docs enabled
    When I run the workflow in a terminal and accept saved flags
    Then only the docs optional effect runs
    And the saved flags enable docs and disable e2e

  Scenario: Explicit CLI flags suppress selection
    Given an isolated planned workflow with docs enabled
    When I run the workflow in a terminal with explicit e2e and docs off
    Then only the e2e optional effect runs
    And no flag selection prompt is displayed

  Scenario: Nonterminal execution preserves saved flags
    Given an isolated planned workflow with docs enabled
    When I run the workflow without a terminal
    Then only the docs optional effect runs
    And no flag selection prompt is displayed

  Scenario: Dry run does not ask or persist choices
    Given an isolated planned workflow with docs enabled
    When I dry run the workflow in a terminal
    Then no workflow effects run
    And no flag selection prompt is displayed
    And the planned session is unchanged

  Scenario: Cancel selection without mutating a planned session
    Given an isolated planned workflow with docs enabled
    When I cancel workflow flag selection
    Then selection fails with a cancellation diagnostic
    And no workflow effects run
    And the planned session is unchanged

  Scenario: Cancel ephemeral exec and clean up its session
    Given an isolated planned workflow with docs enabled
    When I cancel flag selection for an ephemeral exec
    Then selection fails with a cancellation diagnostic
    And no workflow effects run
    And no ephemeral session remains

  Scenario: Default off flags hide single OR group and after-pr candidates
    Given an isolated approval workflow with gated skip candidates
    When I approve with every flag off
    Then approval completed successfully
    And approval asks flags before skips and execution does not ask again
    And the approval skip candidates reflect off flags
    And the approved session records off flags without hidden skips

  Scenario: Selected flags filter skips and are forwarded through Execute now
    Given an isolated approval workflow with gated skip candidates
    When I execute now after selecting e2e
    Then execution reached workspace selection and was cancelled
    And approval asks flags before skips and execution does not ask again
    And the approval skip candidates reflect e2e flags
    And the approved session records e2e flags without hidden skips

  Scenario: Explicit assignments filter skips without either flag prompt
    Given an isolated approval workflow with gated skip candidates
    When I execute now with explicit docs and e2e off
    Then execution reached workspace selection and was cancelled
    And no flag selection prompt is displayed
    And the approval skip candidates reflect docs flags
    And the approved session records docs flags without hidden skips

  Scenario: Remembered enabled skips persist while hidden historical skips are removed
    Given approval history previously skipped enabled and disabled gated steps
    When I approve e2e while accepting its remembered skip
    Then approval completed successfully
    And the approval skip candidates reflect e2e flags
    And only the enabled remembered skip is saved in the session and real history

  Scenario: Cancelling flag selection does not leak a pending toggle
    Given an isolated approval workflow with gated skip candidates
    When I cancel the flag approval selection then accept untouched defaults
    Then approval completed successfully
    And the approved session records off flags without hidden skips

  Scenario: Cancelling skips does not partially commit selected flags
    Given an isolated approval workflow with gated skip candidates
    When I cancel the skip approval selection then accept untouched defaults
    Then approval completed successfully
    And the approved session records off flags without hidden skips

  Scenario: Resume uses persisted choices without prompting again
    Given an isolated planned workflow with docs enabled
    When I resume after an interrupted workflow
    Then only the docs optional effect runs
    And no flag selection prompt is displayed
