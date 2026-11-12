Feature: Planning revision preserves user text and client parity

  Scenario: Replan rejects user text but regenerates without it
    Given a session with a usable plan
    When I replan with feedback and with a question
    Then both requests are rejected with guidance to use fix
    When I replan without user text
    Then regeneration succeeds and preserves the Planned phase

  Scenario: Review actions match backend support in both phases
    Given a session with a usable plan
    When I inspect planning actions in AwaitingApproval and Planned
    And I inspect the CLI list for Replan in Planned as the Fix alias
    Then TUI and WebUI offer Fix, Ask, and Verify in both phases without Replan
    When I Fix and Ask the Planned session
    Then both operations preserve the Planned phase
