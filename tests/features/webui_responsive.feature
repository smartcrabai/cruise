@webui @responsive
Feature: Responsive production WebUI from the October 25 plan
  Production TSX, generated CSS, app.js and HTMX must work together in Chromium.
  Session fixtures are persisted inputs, never mocked HTTP or synthetic DOM.

  @R1
  Scenario: Drawer toggle, backdrop, Escape and hidden keyboard focus
    Given a production WebUI at 375 by 667 pixels
    When I exercise the drawer browser contract
    Then all observable responsive assertions pass

  @R1 @C6 @C7
  Scenario: Navigation closes but cancelled confirmation, polling and settings preserve the drawer
    Given a production WebUI at 375 by 667 pixels
    When I exercise the navigation browser contract
    Then all observable responsive assertions pass

  @R1 @R2
  Scenario: Crossing exactly 768 resets drawer without losing desktop resize persistence
    Given a production WebUI at 1280 by 800 pixels
    When I exercise the desktop browser contract
    Then all observable responsive assertions pass

  @R3 @R4 @R5 @R6 @R7 @R8 @R9
  Scenario Outline: Small screens retain reachable content without document overflow
    Given a production WebUI at <width> by <height> pixels
    When I exercise the layout browser contract
    Then all observable responsive assertions pass
    Examples:
      | width | height |
      | 360   | 640    |
      | 375   | 667    |
      | 767   | 540    |

  @R6
  Scenario: Tall merge preview, publish preview and settings remain reachable
    Given a production WebUI at 360 by 320 pixels
    When I exercise the dialogs browser contract
    Then all observable responsive assertions pass

  @R6 @R8
  Scenario: A live option step has a scrollable dialog and readable text entry
    Given a production WebUI at 375 by 320 pixels
    When I exercise the option browser contract
    Then all observable responsive assertions pass

  @R5
  Scenario: Run All results scroll with the page on mobile and internally on desktop
    Given a production WebUI at 375 by 320 pixels
    When I exercise the runall browser contract
    Then all observable responsive assertions pass
