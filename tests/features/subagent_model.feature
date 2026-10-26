Feature: Task-scoped subagent model overrides reach production execution
  # plan.md sections 2.2, 2.3, 2.4, 3.10 and 4.
  # Local executables replace only the remote SDK boundary, not cruise dispatch.

  Scenario Outline: Step overrides beat workflow values and conflicting Claude environment
    Given this subagent workflow
      """
      sdk: claude
      model: sonnet
      subagent_model: haiku
      env:
        CLAUDE_CODE_SUBAGENT_MODEL: stale
        CLAUDE_CODE_SUBAGENT_MODEL_FORCE: '0'
      steps:
        answer:
          prompt: SUBAGENT_TASK
          subagent_model: opus
          env:
            CLAUDE_CODE_SUBAGENT_MODEL: stale-step
            CLAUDE_CODE_SUBAGENT_MODEL_FORCE: '0'
      """
    When the production CLI runs <purpose>
    Then the CLI succeeds
    And task requests use model <model> and force 1
    Examples:
      | purpose | model |
      | exec    | opus  |
      | plan    | haiku |

  Scenario: Workflow selection is inherited by group prompts
    Given this subagent workflow
      """
      sdk: claude
      subagent_model: haiku
      groups:
        review:
          steps:
            inner:
              prompt: SUBAGENT_GROUP
      steps:
        check:
          group: review
      """
    When the production CLI runs exec
    Then the CLI succeeds
    And task requests use model haiku and force 1

  Scenario: Parallel prompt child selection overrides the workflow default
    Given this subagent workflow
      """
      sdk: claude
      subagent_model: haiku
      steps:
        checks:
          parallel:
            review:
              prompt: SUBAGENT_PARALLEL
              subagent_model: opus
            command:
              command: echo COMMAND_CHILD
      """
    When the production CLI runs exec
    Then the CLI succeeds
    And task requests use model opus and force 1

  Scenario: Omission preserves existing Claude environment including FORCE
    Given this subagent workflow
      """
      sdk: claude
      env:
        CLAUDE_CODE_SUBAGENT_MODEL: legacy
        CLAUDE_CODE_SUBAGENT_MODEL_FORCE: '0'
      steps:
        answer:
          prompt: SUBAGENT_LEGACY
      """
    When the production CLI runs exec
    Then the CLI succeeds
    And task requests use model legacy and force 0

  Scenario: Planning applies workflow selection but title generation excludes it
    Given this subagent workflow
      """
      sdk: claude
      subagent_model: haiku
      steps:
        answer:
          prompt: SUBAGENT_EXEC_ONLY
          subagent_model: opus
      """
    When the production CLI runs plan
    Then the CLI succeeds
    And task requests use model haiku and force 1
    And title requests exclude the subagent override

  Scenario: Workflow generation excludes task subagent selection
    Given this subagent workflow
      """
      sdk: claude
      subagent_model: haiku
      steps:
        answer:
          prompt: SUBAGENT_NOT_FOR_GENERATOR
      """
    And Claude returns a valid generated workflow
    When the production CLI runs generate
    Then the CLI succeeds
    And task requests use model unset and force unset
    And a validated generated workflow is written

  Scenario: Explicit Claude namespace matches a fallback from the default jcode SDK
    Given this subagent workflow
      """
      model: [openai/missing, {sdk: claude, model: sonnet}]
      subagent_model: {sdk: claude, model: haiku}
      retry:
        base_delay_ms: 1
        max_delay_ms: 1
      steps:
        answer:
          prompt: SUBAGENT_FALLBACK
      """
    When the production CLI runs exec
    Then the CLI succeeds
    And SDK dispatch order is jcode,claude
    And task requests use model haiku and force 1

  Scenario: A jcode subagent selection does not leak into a Claude fallback attempt
    Given this subagent workflow
      """
      sdk: jcode
      model: [openai/missing, {sdk: claude, model: sonnet}]
      subagent_model: openai/worker
      retry:
        base_delay_ms: 1
        max_delay_ms: 1
      steps:
        answer:
          prompt: SUBAGENT_NAMESPACE
      """
    When the production CLI runs exec
    Then the CLI succeeds
    And SDK dispatch order is jcode,claude
    And task requests use model unset and force unset

  Scenario: Compilation preserves workflow defaults and expanded phase overrides
    Given this subagent workflow
      """
      sdk: claude
      subagent_model: haiku
      groups:
        g:
          steps:
            inner:
              prompt: inspect
              subagent_model: opus
      steps:
        check:
          group: g
      after-pr:
        review:
          prompt: review
          subagent_model: sonnet
      """
    When production validation and compilation run
    Then compiled group and after-pr prompts preserve explicit selections

  Scenario Outline: Invalid values are rejected for the intended subagent reason
    Given this subagent workflow
      """
      sdk: <sdk>
      subagent_model: <entry>
      steps:
        answer:
          prompt: inspect
      """
    When production validation and compilation run
    Then subagent validation rejects it before SDK dispatch
    Examples:
      | sdk      | entry                          |
      | claude   | [haiku, opus]                  |
      | claude   | {primary: haiku, fallback: opus}|
      | claude   | '   '                          |
      | claude   | haiku:high                     |
      | jcode    | openai/                        |
      | opencode | haiku                          |
      | jcode    | {sdk: claude, model: haiku}     |
      | claude   | {sdk: unknown, model: haiku}    |
      | claude   | {sdk: opencode, model: worker} |

  Scenario: Command backend rejects subagent selection
    Given this subagent workflow
      """
      command: [sh, '-c', 'echo forbidden']
      subagent_model: haiku
      steps:
        answer:
          prompt: inspect
      """
    When production validation and compilation run
    Then subagent validation rejects it before SDK dispatch

  Scenario: Executor rejects command selection even when config validation is bypassed
    Given this subagent workflow
      """
      command: [sh, '-c', 'echo forbidden']
      subagent_model: haiku
      steps:
        answer:
          prompt: inspect
      """
    When the Executor directly runs the command backend
    Then subagent validation rejects it before SDK dispatch

  Scenario Outline: Structural step wrappers cannot own a subagent override
    Given this subagent workflow
      """
      sdk: claude
      groups:
        checks:
          steps:
            inner:
              prompt: inspect
      steps:
        answer:
          <body>
          subagent_model: haiku
      """
    When production validation and compilation run
    Then subagent validation rejects it before SDK dispatch
    Examples:
      | body                                     |
      | option: [{label: finish, next: end}]      |
      | parallel: {child: {prompt: inspect}}      |
      | group: checks                            |
      | workflow_call: ./callee.yaml             |

  Scenario: Non-prompt command steps reject subagent selection
    Given this subagent workflow
      """
      sdk: claude
      steps:
        answer:
          command: echo forbidden
          subagent_model: haiku
      """
    When production validation and compilation run
    Then subagent validation rejects it before SDK dispatch
