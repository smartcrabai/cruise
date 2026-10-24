@workflow_flags
Feature: Select optional workflow flags at the start of a CLI run
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

  Scenario: Resume uses persisted choices without prompting again
    Given an isolated planned workflow with docs enabled
    When I resume after an interrupted workflow
    Then only the docs optional effect runs
    And no flag selection prompt is displayed
