Feature: Verify a plan's generated formal specifications
  Verification is read-only and reports tool results without repairing the plan.

  Scenario: A response without formal specifications is not a pass
    Given a saved plan and a response without formal specifications
    When the generated response is verified
    Then the verification report is error
    And the report explains that nothing was verified
    And the saved plan is unchanged

  Scenario: All Quint stages succeed
    Given a Quint specification with a verification target
    When parse, typecheck, run and verify pass
    Then the verification report is passed

  Scenario: Quint type error short-circuits later stages
    Given a Quint specification with a verification target
    When typecheck returns an error
    Then run and verify are not invoked
    And the verification report is error

  Scenario: Alloy counterexample is a failure
    Given an Alloy check with a satisfying counterexample receipt
    When the Alloy command is verified
    Then the verification report is failed

  Scenario: Cancellation stops verification
    Given a running formal verification process
    When verification is cancelled
    Then verification is interrupted and its process group is stopped

  Scenario: Application verification without specifications preserves the plan
    Given a saved application plan and a backend response without specifications
    When the application verifies the saved plan
    Then the application reports that nothing was verified
    And the application preserves the saved plan and makes one backend call

  Scenario: Application verification runs the generated Quint specification
    Given a saved application plan and a backend response with a Quint specification
    When the application verifies the saved plan
    Then the application runs all Quint verification stages
    And the application preserves the saved plan and interprets the report
