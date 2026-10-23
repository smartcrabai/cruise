# Reconstructed property IDs evidenced by tests/graph_state.rs, not recovered audit labels.
@checkpoint @safe_resume
Feature: Authoritative checkpoint workflow contracts
  Checkpoints gate subsequent effects but do not promise exactly-once effects across crashes.

  Background:
    Given an isolated checkpoint workspace

  @CP-01 @accepted_traversals @ceiling_retention
  Scenario: Lowering a retry ceiling never erases accepted traversal history
    When I compile the retrying shell workflow
    Then accepted counts survive repeated resumes and lowered ceilings

  @CP-02 @pending_transition @source_effect
  Scenario: Resume accepts a pending transition without repeating its source effect
    When I compile the retrying shell workflow
    Then a pending retry resumes without rerunning its source effect

  @CP-03 @checkpoint_gate @not_exactly_once
  Scenario: Failed persistence gates the next shell side effect
    When I compile the two-effect shell workflow
    Then a failed checkpoint prevents the second effect

  @CP-04 @skip_fallback @unbudgeted
  Scenario: Skipping records a traversal without consuming retry budget
    When I compile the skipped shell workflow
    Then the skipped traversal is counted without budget or overflow

  @CP-05 @checkpoint_authority @reposition
  Scenario: Checkpoint position and runtime outvote stale session display
    When I compile the two-effect shell workflow
    Then checkpoint authority and explicit reposition preserve history

  @CP-06 @rejection @preserve_bytes
  Scenario: Unsafe checkpoints are rejected without rewriting evidence
    When I compile the two-effect shell workflow
    Then invalid legacy mismatched and incomplete checkpoints preserve their bytes

  @CP-07 @group_exhaustion @runtime_snapshots
  Scenario: Group retries retain counts and snapshots through resume and exhaustion
    When I compile the grouped shell workflow
    Then group counts and runtime snapshots survive resume and exhaustion

  @CP-08 @stable_topology @completed_resume
  Scenario: Retry policy does not unfold topology and completed resume has no effects
    When I compile the two-effect shell workflow
    Then topology stays stable and completed resume runs no effects
