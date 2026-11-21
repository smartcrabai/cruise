Feature: Recover interrupted or failed exec sessions in the real CLI
  Each scenario runs in a fresh repository and uses a real local daemon.

  Scenario: Retry by ID even after clean and daemon restart
    When the production recovery fixture "resume-by-id" runs
    Then the production fixture passes all its recovery assertions

  Scenario: Resume by selecting the recovery action in the list menu
    When the production recovery fixture "menu-resume" runs
    Then the production fixture passes all its recovery assertions

  Scenario: Delete immediately from the list menu without modifying user files
    When the production recovery fixture "menu-delete" runs
    Then the production fixture passes all its recovery assertions

  Scenario: Preview and live retry reject damaged or missing started checkpoints
    When the production recovery fixture "damaged-checkpoint" runs
    Then the production fixture passes all its recovery assertions

  Scenario: A handled failure transfers control without retaining a failed session
    When the production recovery fixture "handled-failure" runs
    Then the production fixture passes all its recovery assertions

  Scenario: The wrong branch is rejected without consuming the retry point
    When the production recovery fixture "wrong-branch" runs
    Then the production fixture passes all its recovery assertions

  Scenario: Workspace setup failure retries from graph entry
    When the production recovery fixture "workspace-setup" runs
    Then the production fixture passes all its recovery assertions

  Scenario: Preview and live retry honor the saved explicit retry budget
    When the production recovery fixture "saved-budget" runs
    Then the production fixture passes all its recovery assertions

  Scenario: A hard error does not supersede an earlier unresolved ordinary failure
    When the production recovery fixture "hard-error" runs
    Then the production fixture passes all its recovery assertions

  Scenario: A rejected pending transition retains its source and retry budget
    When the production recovery fixture "pending-transition" runs
    Then the production fixture passes all its recovery assertions

  Scenario: An interrupted terminal phase write still permits retry of the unresolved step
    When the production recovery fixture "interrupted-terminal-write" runs
    Then the production fixture passes all its recovery assertions

  Scenario: Mid-graph suspension resumes at the live step and remembers earlier failure
    When the production recovery fixture "mid-graph-suspension" runs
    Then the production fixture passes all its recovery assertions

  Scenario: Clean retains failed exec artifacts until successful recovery
    When the production recovery fixture "artifact-retention" runs
    Then the production fixture passes all its recovery assertions

  Scenario: Recovery preview lists the expanded failed child and following steps
    When the production recovery fixture "expanded-child-preview" runs
    Then the production fixture passes all its recovery assertions

  Scenario: Ctrl+C preserves Failed before admission and saves Suspended afterward
    When the production recovery fixture "admission-interrupt" runs
    Then the production fixture passes all its recovery assertions
