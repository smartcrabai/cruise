Feature: Per-user daemon lifecycle and API
  The CLI manages one daemon per state directory; API requests use its bearer token.

  Scenario: Start, inspect, and stop a background daemon
    Given an isolated Cruise environment
    When I start the daemon on an ephemeral port
    Then status reports the running daemon and its WebUI responds
    When I stop the daemon
    Then status reports that the daemon is not running

  Scenario: A repeated start preserves the existing instance
    Given an isolated Cruise environment
    When I start the daemon on an ephemeral port
    And I start the daemon again without options
    Then the existing instance remains registered
    And a foreground daemon cannot start concurrently

  Scenario: Restart replaces the process and preserves its port
    Given an isolated Cruise environment
    When I start the daemon on an ephemeral port
    And I restart the daemon
    Then a new daemon is running on the original port

  Scenario: A session client autostarts the daemon
    Given an isolated Cruise environment
    When I run the list client without starting the daemon
    Then the client has started and registered the daemon

  Scenario: The daemon exits when its registry record disappears
    Given an isolated Cruise environment
    When I start the daemon on an ephemeral port
    And I remove the daemon registry record
    Then the daemon exits after registry removal

  Scenario: An unspecified busy default port falls back to an ephemeral port
    Given an isolated Cruise environment
    When I start the daemon without a port while the default port is busy
    Then the daemon is registered on an ephemeral port

  Scenario: An occupied explicit port fails strictly
    Given an isolated Cruise environment
    When I start the daemon on an occupied explicit port
    Then startup fails without publishing a registry record

  Scenario: A stale registry version does not override the running process version
    Given an isolated Cruise environment
    When I start the daemon on an ephemeral port
    And I change the registered daemon version
    And I run the list client
    Then the client keeps the current daemon and status reports its process version

  Scenario: API status and shutdown require the daemon bearer token
    Given an isolated Cruise environment
    When I start the daemon on an ephemeral port
    And I request API status without a bearer token
    Then API status rejects the request
    When I request API status with the registered bearer token
    Then API status returns success
    When I send an unauthenticated API shutdown request
    Then shutdown is rejected and the daemon remains running
    When I send an authenticated API shutdown request
    Then shutdown is accepted and the daemon exits
