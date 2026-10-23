@retry-barrier
Feature: Negative control for production assertion failure
  Scenario: A wrong observable prompt count cannot pass or be retried
    Given a pinned V2 server exercising complete
    When the real OpenCode Executor runs with two retries and a fallback model
    Then exactly 2 provider prompts were submitted
