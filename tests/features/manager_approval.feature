Feature: Manager proposal approval is explicit and revision-bound

  Scenario: Preview and stale approvals do not authorize work
    Given an isolated repository with a manager proposal
    When the proposal is replaced by a newer preview
    And approvals are attempted with the old identity and an old revision
    Then the newer preview remains and no goal or child attempt exists

  Scenario: New approval is unavailable while execution is incomplete
    Given an isolated repository with a manager proposal
    When approval of the current proposal is attempted twice
    Then the proposal remains available and no goal or child attempt exists
