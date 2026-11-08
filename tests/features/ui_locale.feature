Feature: UI language selection
  Scenario: Environment overrides a saved preference
    Given a saved UI language "fr"
    And the environment UI language is "ja"
    When I inspect the UI language configuration
    Then the effective UI language is "ja" from "env"

  Scenario: Automatic preference uses the system locale
    Given a saved UI language "auto"
    And the system locale is "ko_KR.UTF-8"
    When I inspect the UI language configuration
    Then the effective UI language is "ko" from "system"

  Scenario: Unsupported environment value falls back to system locale or English
    Given the environment UI language is "xx"
    When I inspect the UI language configuration
    Then the effective UI language is the platform fallback
    And a warning mentions "CRUISE_UI_LANGUAGE='xx'"

  Scenario: Invalid environment falls through to saved preference
    Given a saved UI language "fr"
    And the environment UI language is "xx"
    And the system locale is "ko_KR.UTF-8"
    When I inspect the UI language configuration
    Then the effective UI language is "fr" from "config"
    And a warning mentions "CRUISE_UI_LANGUAGE='xx'"

  Scenario: Saved preference survives a new process and auto clears it
    Given a saved UI language "ja"
    When I inspect the UI language configuration
    Then the effective UI language is "ja" from "config"
    When I set the UI language to "auto"
    And I inspect the UI language configuration again
    Then the effective UI language is the platform fallback

  Scenario: Invalid setting is rejected without changing the saved preference
    Given a saved UI language "fr"
    When I inspect the UI language configuration
    And I attempt to set the UI language to "xx"
    Then the language setting is rejected
    When I inspect the UI language configuration again
    Then the effective UI language is "fr" from "config"

  Scenario: System language is reflected in real CLI output
    Given the system locale is "ko_KR.UTF-8"
    When I list sessions
    Then the output contains "세션을 찾을 수 없습니다."
