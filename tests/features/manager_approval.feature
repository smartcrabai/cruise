Feature: Manager proposal approval is explicit and revision-bound

  Scenario: Preview and stale approvals do not authorize work
    Given an isolated repository with a manager proposal
    When the proposal is replaced by a newer preview
    And approvals are attempted with the old identity and an old revision
    Then the newer preview remains and no goal or child attempt exists

  Scenario: Approval is durable and idempotent
    Given an isolated repository with a manager proposal
    When the current proposal is approved twice
    Then one active goal exists for that exact proposal at its frozen target tip without child attempts
