Feature: Wrong Jcode fallback order cannot pass
  Scenario: An intentionally wrong candidate order fails after production compilation
    Given a Jcode workflow with model candidates "claude/claude-opus-4-5" then "openai/gpt-5"
    When production validation and compilation run
    Then a deliberately wrong Jcode candidate order is rejected
