Feature: Pending authentication contracts must fail
  Scenario: Unimplemented contract cannot silently pass
    Then the deliberately pending auth contract is implemented
