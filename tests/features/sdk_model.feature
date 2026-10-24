Feature: SDK-qualified model lists execute through production boundaries
  # plan.md sections 3.2, 3.3, 3.5 and completion conditions 1-3.
  # Existing OpenCode support is unchanged. jcode and claude are representative.

  Scenario Outline: Mixed candidates return to the workflow SDK during execution and planning
    Given an isolated workflow with a mixed SDK list for <purpose>
    When the real CLI runs <purpose> against refusing local SDK processes
    Then SDK subprocesses ran in order jcode claude jcode
    And each subprocess received its selected model and Claude received the original request
    And both SDK switch notices identify the source and destination
    Examples:
      | purpose |
      | exec    |
      | plan    |

  Scenario: Same model names distinguish SDK owners but not effort variants within one SDK
    Given a mixed SDK list sharing model names with different effort suffixes
    When the real CLI runs exec against refusing local SDK processes
    Then different SDK owners execute but the repeated jcode effort variant does not

  Scenario Outline: Resume identifiers are qualified by their actual SDK owner
    Given a local Claude process and a saved <owner> conversation
    When the production Executor runs a Claude candidate
    Then the Claude resume argument is <behavior>
    And the outcome and checkpoint callback identify the actual Claude conversation
    Examples:
      | owner  | behavior |
      | claude | present  |
      | jcode  | absent   |

  Scenario Outline: Unrelated app settings updates preserve mixed model lists on disk
    Given an app config in <format> with mixed execution and planning candidates
    When production app config loading and atomic saving update parallelism
    Then reloading preserves candidate order SDK and original entry forms
    Examples:
      | format |
      | json   |
      | yaml   |

  Scenario Outline: Command mode rejects SDK objects before starting any agent
    Given a command workflow with an SDK object in <field> at <position>
    When the real CLI validates and executes the conflicting workflow
    Then the command conflict is reported without launching any subprocess
    Examples:
      | field      | position |
      | model      | primary  |
      | model      | fallback |
      | plan_model | primary  |
      | plan_model | fallback |
