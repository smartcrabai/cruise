Feature: Manage sessions from cruise list
  The picker is a terminal interface. Headless JSON remains a session inventory.

  Scenario Outline: Both terminal entry points give an empty workspace a picker
    Given an isolated workflow with no sessions
    When I open "<command>" in a terminal
    Then the empty picker offers New Session, Clean, Run All, and Quit
    When I exit by "<exit>"
    Then no session or workflow activity exists

    Examples:
      | command     | exit   |
      | cruise list | Quit   |
      | cruise      | Escape |

  Scenario: Draft creation waits for task submission and never plans
    Given an isolated workflow with no sessions
    When I open "cruise list" in a terminal
    Then the empty picker offers New Session, Clean, Run All, and Quit
    When I choose "draft" and cancel task entry
    Then the empty picker offers New Session, Clean, Run All, and Quit
    And no session or workflow activity exists
    When I choose "draft" and submit "menu draft witness"
    Then the picker refreshes with "menu draft witness"
    And the only session is a Draft without planner or runner activity
    When I open the initially selected session
    Then its submenu offers Generate Plan
    When I go back from the session submenu
    Then the picker refreshes with "menu draft witness"

  Scenario: Selecting between duplicate row labels routes by session identity
    Given an isolated workflow with no sessions
    And two planned sessions with identical row labels but different plans
    When I open "cruise list" in a terminal
    Then the picker refreshes with "Duplicate title"
    When I open the next session row
    Then the submenu displays the second session's plan

  Scenario: Interactive planning reaches approval even with force_exec enabled
    Given an isolated workflow with no sessions
    When I open "cruise list" in a terminal
    Then the empty picker offers New Session, Clean, Run All, and Quit
    When I choose "interactive" and submit "interactive plan witness"
    And I release the fixture planner
    Then the plan awaits approval without direct execution
    When I approve the plan without skipping any steps
    Then the picker refreshes with "Planned"
    And the approved session is Planned without runner activity

  Scenario: Background planning returns to the picker while planning is held
    Given an isolated workflow with no sessions
    When I open "cruise list" in a terminal
    Then the empty picker offers New Session, Clean, Run All, and Quit
    When I choose "background" and submit "stdin"
    Then the picker refreshes with "stdin"
    And the held background task is literally stdin and has no plan yet
    When I release the fixture planner
    Then that session's plan is written and awaits approval

  Scenario: Clean defaults to No and refreshes after removing the last session
    Given an isolated workflow with no sessions
    And a completed cleanup-eligible session
    When I open "cruise list" in a terminal
    Then the picker refreshes with "terminal-clean-witness"
    When I select Clean and accept the default answer
    Then the picker refreshes with "terminal-clean-witness"
    And the completed session still exists
    When I select Clean and confirm
    Then the empty picker offers New Session, Clean, Run All, and Quit
    And no session or workflow activity exists

  Scenario: Run All without eligible sessions does not start work
    Given an isolated workflow with no sessions
    And an ineligible draft session
    When I open "cruise list" in a terminal
    Then the picker refreshes with "zzz-ineligible-draft-witness"
    When I select Run All
    Then there is a no-work message and the picker returns
    And the draft is unchanged and no workflow started

  Scenario: Run All requires confirmation and runs only eligible sessions
    Given an isolated workflow with no sessions
    And an ineligible draft session
    And an eligible planned session
    When I open "cruise list" in a terminal
    Then the picker refreshes with "eligible-run-witness"
    When I select Run All
    And I accept the default Run All answer
    Then the picker refreshes with "eligible-run-witness"
    And the planned session is unchanged
    And the draft is unchanged and no workflow started
    When I select Run All
    And I confirm Run All
    Then the batch summary appears and the picker refreshes
    And only the planned session ran to completion

  Scenario: Empty headless JSON contains no picker output
    Given an isolated daemon whose inventory is "empty"
    When I run headless cruise list --json
    Then stdout is an empty JSON array
    And stderr contains no picker prompts

  Scenario: Populated headless JSON matches the authenticated sessions API
    Given an isolated daemon whose inventory is "populated"
    When I run headless cruise list --json
    And I request the authenticated sessions API
    Then the CLI and API expose the same session rows without action rows
    And stderr contains no picker prompts
