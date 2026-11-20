Feature: Manager proposal approval is explicit and revision-bound

  Scenario: Preview and stale approvals do not authorize work
    Given an isolated repository with a manager proposal
    When the proposal is replaced by a newer preview
    And approvals are attempted with the old identity and an old revision
    Then the newer preview remains and no goal or child attempt exists

  Scenario: Approval is refused while execution is unavailable
    Given an isolated repository with a manager proposal
    When approval of the current proposal is attempted twice
    Then the current preview remains and no goal or child attempt exists
