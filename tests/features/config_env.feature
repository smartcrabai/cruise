Feature: Environment overrides for resolved configuration
  Scenario: An app override affects display but is not saved
    Given an isolated Cruise project
    And CRUISE_RUN_ALL_PARALLELISM is set to "4"
    When I run cruise config
    Then the command succeeds and reports "run_all_parallelism = 4"
    When I run cruise config setting parallelism to "2"
    Then the command succeeds
    When I clear the environment override and run cruise config
    Then the command succeeds and reports "run_all_parallelism = 2"

  Scenario Outline: Invalid values identify the environment variable
    Given an isolated Cruise project
    And <variable> is set to "<value>"
    When I run cruise <command>
    Then the command fails and reports "<variable>"

    Examples:
      | variable                    | value | command |
      | CRUISE_RUN_ALL_PARALLELISM | 0     | config  |
      | CRUISE_RUN_ALL_PARALLELISM | +1    | config  |
      | CRUISE_MAX_RETRIES          | -1    | exec    |
      | CRUISE_CLEANUP_AFTER_PR     | yes   | exec    |

  Scenario: Command and SDK overrides cannot be combined
    Given an isolated Cruise project
    And CRUISE_SDK is set to "jcode"
    And CRUISE_COMMAND is set to "[]"
    When I run cruise exec
    Then the command fails and reports "mutually exclusive"
    And the output mentions "CRUISE_SDK"
    And the output mentions "CRUISE_COMMAND"
