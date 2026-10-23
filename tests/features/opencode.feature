@opencode @v2
Feature: Owned OpenCode V2 execution through Cruise's real Executor
  The plan requires terminal execution evidence, durable recovery without replay,
  and a retry barrier independent of visible output. HTTP/SSE are pinned fixtures
  and the owned shell CLI exercises actual process startup and reaping.

  @terminal @durable-recovery
  Scenario Outline: Successful execution produces one authoritative answer
    Given a pinned V2 server exercising <case>
    When the real OpenCode Executor runs with two retries and a fallback model
    Then the answer is returned exactly once without foreign or reasoning text
    And exactly 1 provider prompts were submitted
    And the owned CLI has been reaped before the Executor returns

    Examples:
      | case       |
      | complete   |
      | disconnect |

  @retry-barrier @no-replay
  Scenario Outline: Unsafe failures cannot retry or switch models even without streamed output
    Given a pinned V2 server exercising <case>
    When the real OpenCode Executor runs with two retries and a fallback model
    Then the execution fails rather than reporting acceptance or text as success
    And exactly 1 provider prompts were submitted
    And the owned CLI has been reaped before the Executor returns

    Examples:
      | case                 |
      | incomplete           |
      | log-unavailable      |
      | malformed            |
      | uncertain-acceptance |
      | tool-effect          |

  @safe-retry @fallback
  Scenario Outline: Proven safe provider failures retain the shared retry path
    Given a pinned V2 server exercising <case>
    When the real OpenCode Executor runs with two retries and a fallback model
    Then the answer is returned exactly once without foreign or reasoning text
    And exactly 2 provider prompts were submitted
    And the owned CLI has been reaped before the Executor returns

    Examples:
      | case               |
      | safe-rate-limit    |
      | safe-model-missing |

  @plugin @fail-closed
  Scenario: Missing mandatory native plugin fails before any prompt is submitted
    Given a pinned V2 server exercising plugin-disabled
    When the real OpenCode Executor runs with two retries and a fallback model
    Then the execution fails rather than reporting acceptance or text as success
    And exactly 0 provider prompts were submitted
    And the owned CLI has been reaped before the Executor returns

  @checkpoint @fail-closed
  Scenario: Refused early conversation checkpoint prevents prompt submission
    Given a pinned V2 server exercising checkpoint-refused
    When the real OpenCode Executor runs with two retries and a fallback model
    Then the execution fails rather than reporting acceptance or text as success
    And exactly 0 provider prompts were submitted
    And the owned CLI has been reaped before the Executor returns

  @cancellation @no-replay
  Scenario: Cancelling an active turn interrupts it and reaps the owned process
    Given a pinned V2 server exercising cancelled
    When the real OpenCode Executor runs with two retries and a fallback model
    Then the active session was interrupted without converting cancellation into success
    And exactly 1 provider prompts were submitted
    And the owned CLI has been reaped before the Executor returns
