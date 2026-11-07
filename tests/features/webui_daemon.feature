Feature: WebUI process lifecycle
  The CLI manages one WebUI per state directory without exposing its control token.

  Scenario: Start, inspect, and stop a background WebUI
    Given an isolated Cruise environment
    When I start the WebUI on an ephemeral port
    Then status reports the running daemon and its endpoint responds
    When I stop the WebUI
    Then status reports that the WebUI is not running

  Scenario: A repeated start preserves the existing instance
    Given an isolated Cruise environment
    When I start the WebUI on an ephemeral port
    And I start the WebUI again without options
    Then the existing instance remains registered
    And a foreground WebUI cannot start concurrently

  Scenario: Restart replaces the process and preserves its port
    Given an isolated Cruise environment
    When I start the WebUI on an ephemeral port
    And I restart the WebUI
    Then a new daemon is running on the original port

  Scenario: Stop requests require the private control token
    Given an isolated Cruise environment
    When I start the WebUI on an ephemeral port
    And I send an unauthenticated control stop request
    Then the control request is rejected and the daemon remains running
