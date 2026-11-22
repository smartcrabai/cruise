Feature: Workflow-scoped read-only jcode provider authentication preflight
  # Non-dry-run session commands use the CLI client and resident daemon; run rejection occurs synchronously at POST /api/v1/sessions/{id}/run.
  # Only the final jcode process is a refusal fixture.

  Scenario Outline: Expired access without refresh rejects every production entrypoint
    Given isolated expired credentials and a <selection> workflow
    When the CLI client invokes <entry>
    Then expired authentication stops before SDK dispatch and workspace creation
    And credentials remain byte identical without locks and secrets stay private
    Examples:
      | entry | selection |
      | plan  | jcode     |
      | exec  | jcode     |
      | run   | jcode     |
      | plan  | openai    |
      | exec  | mixed     |
      | plan  | planning-only |
      | exec  | execution-only |
      | exec  | parallel  |
      | exec  | group     |
      | exec  | after-pr  |
      | plan  | fallback  |

  Scenario Outline: Refreshable and unknown credentials pass to real dispatch
    Given isolated <credentials> credentials and a <selection> workflow
    When the CLI client invokes <entry>
    Then preflight passes to the observable refusing SDK boundary
    And credentials remain byte identical without locks and secrets stay private
    Examples:
      | credentials | selection | entry |
      | refreshable | jcode     | exec  |
      | missing     | jcode     | plan  |
      | malformed   | jcode     | exec  |
      | multi-renewable | jcode | exec  |
      | multi-unknown   | jcode | plan  |
      | multi-renewable | openai | plan |
      | multi-unknown   | openai | exec |

  Scenario: Expired jcode provider targets are aggregated and duplicates produce one message per provider
    Given isolated expired credentials and a both-expired workflow
    When the CLI client invokes exec
    Then expired authentication stops before SDK dispatch and workspace creation
    And both expired providers have exactly one actionable login message
    And credentials remain byte identical without locks and secrets stay private

  Scenario Outline: Dry runs never require credentials
    Given isolated expired credentials and a jcode workflow
    And dry run is requested
    When the CLI client invokes <entry>
    Then dry run succeeds without dispatch or authentication rejection
    And credentials remain byte identical without locks and secrets stay private
    Examples:
      | entry |
      | plan  |
      | run   |
      | exec  |

  Scenario: Explicit optout bypasses known expired credentials
    Given isolated expired credentials and a jcode workflow
    And the auth preflight optout is enabled
    When the CLI client invokes exec
    Then preflight passes to the observable refusing SDK boundary
    And credentials remain byte identical without locks and secrets stay private

  Scenario: Process API credentials bypass expired provider OAuth tokens
    Given isolated expired credentials and a jcode workflow
    And provider API authentication is present in the child environment
    When the CLI client invokes exec
    Then preflight passes to the observable refusing SDK boundary
    And credentials remain byte identical without locks and secrets stay private

  Scenario: Workflow API credentials bypass expired provider OAuth tokens
    Given isolated expired credentials and a api-env workflow
    When the CLI client invokes plan
    Then preflight passes to the observable refusing SDK boundary
    And credentials remain byte identical without locks and secrets stay private

  Scenario Outline: Unused scope credentials do not reject an unrelated workflow phase
    Given isolated expired credentials and a <selection> workflow
    When the CLI client invokes <entry>
    Then preflight passes to the observable refusing SDK boundary
    And credentials remain byte identical without locks and secrets stay private
    Examples:
      | selection      | entry |
      | planning-only  | exec  |
      | execution-only | plan  |
