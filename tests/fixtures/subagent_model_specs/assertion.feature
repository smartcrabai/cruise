Feature: Wrong subagent model assertions cannot silently pass
  Scenario: An intentionally wrong local selection is rejected after compilation
    Given a Jcode subagent workflow with global model "openai/global" and step model "openai/worker"
    When production validation and compilation run
    Then the step subagent model is "deliberately-wrong"
