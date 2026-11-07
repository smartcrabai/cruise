Feature: Provider installation CLI contracts
  Scenario: Status is read-only and reports pinned providers without running them
    Given isolated provider binaries on PATH
    When I run provider status
    Then status reports all pinned providers
    And no provider ran and no data directory was created

  Scenario Outline: Invalid install arguments fail without writing data
    Given isolated provider binaries on PATH
    When I run install with <arguments>
    Then the install command fails
    And no provider ran and no data directory was created
    Examples:
      | arguments          |
      | --force            |
      | --status --force   |
      | jcode --status     |
      | unknown            |
      | jcode claude       |

  Scenario: An invalid explicit override does not silently fall back to PATH
    Given isolated provider binaries on PATH
    And the jcode override points to a missing file
    When I run provider status
    Then the jcode status reports an override error
    And no provider ran and no data directory was created
