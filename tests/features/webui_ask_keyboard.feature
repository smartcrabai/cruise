@webui @ask_keyboard
Feature: Planning answers through the keyboard
  Scenario Outline: Submit the reloaded question with <modifier>+Enter
    When I run a real command workflow in Chromium and exercise the <modifier> shortcut on its Planning question
    Then reload preserves the pending question, plain Enter inserts a newline, and empty answers are blocked
    And the Tell editor remains independent of the Planning shortcut
    And the intercepted answer POST fails without replay and only the explicit retry reaches the daemon with the original request ID and text and clears the question

    Examples:
      | modifier |
      | Meta     |
      | Control  |

  Scenario: Retry Send answer after a failed POST
    When I run a real command workflow in Chromium and exercise Send answer recovery on its Planning question
    Then the controls re-enable and retry the original answer to the daemon
