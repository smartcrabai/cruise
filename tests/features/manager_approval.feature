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

  Scenario: Retrying verification permanently discards an unapproved corrective preview
    Given a blocked Manager goal with a failed check and an unapproved corrective preview
    When the operator retries goal verification
    Then the retry archives the failed verification and the preview without authorizing work
    And Manager status no longer displays the unapproved preview
    When verification fails again at the same integrated tip
    Then approval of the discarded identity and revision is stale
    When the operator saves and approves a fresh corrective preview
    Then only the fresh revision authorizes corrective work and the discarded approval remains stale

  Scenario: Retrying without a pending preview does not create a discard entry
    Given a blocked Manager goal with a failed check but no corrective preview
    When the operator retries goal verification
    Then the retry archives the failed verification without creating a discard entry

  Scenario: A rejected retry leaves the preview and history intact
    Given a paused Manager goal with a pending corrective preview
    When the operator attempts to retry goal verification
    Then the rejected retry leaves the goal unchanged
