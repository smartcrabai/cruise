Feature: Jcode subagent model selection
  Scenario: A step model string overrides the workflow string
    Given a Jcode subagent workflow with global model "openai/global" and step model "openai/worker"
    When production validation and compilation run
    Then the step subagent model takes precedence

  Scenario Outline: Invalid subagent models reject before prompt dispatch
    Given a Jcode workflow with subagent value "<value>"
    When production validation and compilation run
    Then subagent validation rejects it before prompt dispatch
    Examples:
      | value       |
      | blank       |
      | haiku:high  |
      | openai/     |

  Scenario: Command mode rejects subagent selection
    Given a command workflow with subagent value "openai/worker"
    When production validation and compilation run
    Then subagent validation rejects it before prompt dispatch
