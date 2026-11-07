Feature: Instructions for an active cruise run
  An operator can queue instructions without changing workflow control flow.

  Scenario: An instruction waits through a command and is delivered once
    Given a run paused in a command before two agent steps
    When the operator tells the run "check the migration"
    Then the command has not leased the instruction
    When the command completes
    Then the first agent receives the instruction before the commit guard
    And the second agent does not receive the instruction
    And the instruction is leased and delivered exactly once

  Scenario: An instruction is rejected for a planned session
    Given a planned session
    When the operator tells the run "check the migration"
    Then the command rejects the instruction without changing the inbox

  Scenario: A failed agent invocation releases the instruction
    Given a run paused in a command before a failing agent step
    When the operator tells the run "check the migration"
    And the agent executable becomes unavailable
    And the command completes
    Then the instruction is released without being delivered
