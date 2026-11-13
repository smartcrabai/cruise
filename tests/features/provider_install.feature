Feature: Provider installation CLI contracts
  Scenario: Status is read-only and reports no latest-version claim
    Given isolated provider binaries on PATH
    When I run provider status
    Then status reports no installed versions and uses PATH
    And no provider ran and no data directory was created

  Scenario: A valid older recorded version remains the managed provider
    Given isolated provider binaries on PATH
    And a valid older recorded jcode installation
    When I run provider status
    Then status reports the recorded older jcode version as managed
    And no provider ran

  Scenario: A damaged managed binary falls back to PATH
    Given isolated provider binaries on PATH
    And the recorded jcode binary has been damaged
    When I run provider status
    Then status reports the damaged jcode install and falls back to PATH
    And no provider ran

  Scenario: An explicit override takes precedence over managed and PATH
    Given isolated provider binaries on PATH
    And a valid older recorded jcode installation
    And the jcode override points to a valid file
    When I run provider status
    Then status reports the jcode override as effective
    And no provider ran

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
