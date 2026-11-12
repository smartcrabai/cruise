Feature: Base branch CI gate for fresh workflow starts
  A new workflow must not start when the base branch is broken or unverifiable.

  Scenario Outline: GitHub rollup states gate a fresh run
    Given an isolated GitHub session with a <state> rollup
    When I start the workflow
    Then the base CI query is made
    And the base CI outcome is <outcome>

    Examples:
      | state   | outcome  |
      | FAILURE | rejected |
      | SUCCESS | allowed  |
      | PENDING | allowed  |
      | NONE    | allowed  |
      | INVALID | rejected |

  Scenario: An unavailable forge fails closed
    Given an isolated GitHub session with a ERROR rollup
    When I start the workflow
    Then the base CI query is made
    And the base CI outcome is rejected

  Scenario: The gate can be disabled in project defaults
    Given an isolated GitHub session with a FAILURE rollup
    And base CI checking is disabled
    When I start the workflow
    Then no base CI query is made
    And the base CI outcome is allowed

  Scenario: Dry runs do not query base CI
    Given an isolated GitHub session with a FAILURE rollup
    When I dry run the workflow
    Then no base CI query is made
    And the base CI outcome is allowed

  Scenario: The configured GitHub base branch is checked
    Given an isolated GitHub session with a SUCCESS rollup
    And the configured base branch is develop
    When I start the workflow
    Then the configured branch is queried
    And the base CI outcome is allowed

  Scenario Outline: GitLab pipeline states gate a fresh run
    Given an isolated GitLab session with a <state> rollup
    When I start the workflow
    Then the base CI query is made
    And the base CI outcome is <outcome>

    Examples:
      | state   | outcome  |
      | success | allowed  |
      | running | allowed  |
      | NONE    | allowed  |
      | failed  | rejected |
