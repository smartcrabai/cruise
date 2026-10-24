Feature: Workflow-scoped read-only agent OAuth preflight
  # plan.md sections 0, 3.1-3.4, 4 and 6. Only the final SDK process is a refusal fixture.
  Scenario Outline: Expired access without refresh rejects every production entrypoint
    Given isolated expired credentials and a <selection> workflow
    When the production CLI enters <entry>
    Then expired authentication stops before SDK dispatch and workspace creation
    And credentials remain byte identical without locks and secrets stay private
    Examples:
      | entry | selection |
      | plan  | jcode     |
      | exec  | jcode     |
      | run   | jcode     |
      | exec  | claude    |
      | plan  | openai    |
      | exec  | mixed     |
      | plan  | planning-only |
      | exec  | execution-only |
      | exec  | parallel  |
      | exec  | group     |
      | exec  | after-pr  |
      | plan  | fallback  |

  Scenario: An expired refresh token rejects even when its secret is present
    Given isolated expired-refresh credentials and a claude workflow
    When the production CLI enters exec
    Then expired authentication stops before SDK dispatch and workspace creation
    And credentials remain byte identical without locks and secrets stay private

  Scenario Outline: Refreshable and unknown credentials pass to real dispatch
    Given isolated <credentials> credentials and a <selection> workflow
    When the production CLI enters <entry>
    Then preflight passes to the observable refusing SDK boundary
    And credentials remain byte identical without locks and secrets stay private
    Examples:
      | credentials | selection | entry |
      | refreshable | jcode     | exec  |
      | refreshable | claude    | plan  |
      | missing     | jcode     | plan  |
      | malformed   | claude    | exec  |
      | multi-renewable | jcode | exec  |
      | multi-unknown   | jcode | plan  |
      | multi-renewable | openai | plan |
      | multi-unknown   | openai | exec |
      | future-access   | claude | exec |

  Scenario: All expired SDK targets are aggregated and duplicate candidates produce one login message per target
    Given isolated expired credentials and a both-expired workflow
    When the production CLI enters exec
    Then expired authentication stops before SDK dispatch and workspace creation
    And both expired providers have exactly one actionable login message
    And credentials remain byte identical without locks and secrets stay private

  Scenario Outline: Dry runs never require credentials
    Given isolated expired credentials and a jcode workflow
    And dry run is requested
    When the production CLI enters <entry>
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
    When the production CLI enters exec
    Then preflight passes to the observable refusing SDK boundary
    And credentials remain byte identical without locks and secrets stay private

  Scenario: Process API credentials bypass Claude OAuth files
    Given isolated expired credentials and a claude workflow
    And Claude API authentication is present in the child environment
    When the production CLI enters exec
    Then preflight passes to the observable refusing SDK boundary
    And credentials remain byte identical without locks and secrets stay private

  Scenario: Workflow API credentials bypass Claude OAuth files
    Given isolated expired credentials and a api-env workflow
    When the production CLI enters plan
    Then preflight passes to the observable refusing SDK boundary
    And credentials remain byte identical without locks and secrets stay private

  Scenario Outline: Unused scope credentials do not reject an unrelated workflow phase
    Given isolated expired credentials and a <selection> workflow
    When the production CLI enters <entry>
    Then preflight passes to the observable refusing SDK boundary
    And credentials remain byte identical without locks and secrets stay private
    Examples:
      | selection      | entry |
      | planning-only  | exec  |
      | execution-only | plan  |
