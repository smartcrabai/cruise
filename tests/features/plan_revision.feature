Feature: Planning revision preserves user text and client parity
  The parity boundary is the CLI session list and the daemon-served WebUI.

  Scenario: Replan rejects user text but regenerates without it
    Given a session with a usable plan
    When the application replans with feedback and with a question
    Then both requests are rejected with guidance to use fix
    When the application replans without user text
    Then regeneration succeeds and preserves the Planned phase

  Scenario: Review actions match backend support in both phases
    Given a session with a usable plan
    When I inspect WebUI actions in AwaitingApproval and Planned
    And I inspect the CLI list for Replan in Planned as the Fix alias
    Then WebUI offers Fix, Ask, and Verify in both phases without Replan
    When the application performs Fix and Ask on the Planned session
    Then both operations preserve the Planned phase
