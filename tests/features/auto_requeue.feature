@auto_requeue
Feature: Batch auto-requeue configuration and startup boundary
  The persisted retry budget applies to batch execution and does not turn old failures into new work.

  Background:
    Given an isolated Cruise installation

  Scenario: The default budget is disabled and configuration persists
    When I inspect the retry budget
    Then the retry budget is 0
    When I set the retry budget to 3
    And I inspect the retry budget
    Then the retry budget is 3

  Scenario: The one-shot batch override is not accepted for a single session
    When I run one session with a retry override
    Then the command is rejected because it requires all

  Scenario: Invalid persisted retry limits fail closed
    Given a negative persisted retry budget
    When I start a batch without an override
    Then the command fails

  Scenario: A previously failed session is not picked up at batch startup
    Given a previously failed session with one recorded retry
    When I start a batch with a retry budget of 2
    Then the session remains failed with one recorded retry

  Scenario Outline: A newly failing batch step respects its retry budget
    Given an independent repository with a planned no-changes failure
    When I run the batch with a retry budget of <limit>
    Then the batch session is failed with <limit> automatic retries

    Examples:
      | limit |
      | 0     |
      | 2     |

  Scenario: An authentication failure does not consume the retry budget
    Given an independent repository with a planned authentication failure
    When I run the batch with a retry budget of 2
    Then the batch session is failed with 0 automatic retries
    And the failure reports authentication
