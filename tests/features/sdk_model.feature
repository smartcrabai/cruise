Feature: Ordered Jcode model fallback strings
  Scenario Outline: Execution and planning compile ordered plain model strings
    Given a Jcode workflow with model candidates "<primary>" then "<fallback>"
    When production validation and compilation run
    Then the compiled candidates preserve that order
    Examples:
      | primary                    | fallback             |
      | claude/claude-opus-4-5:high | openai/gpt-5:medium  |
      | openai/gpt-5                | anthropic/sonnet     |
