Feature: Workflow package bundles
  Scenario: Installing a bundle publishes every listed workflow together
    Given an isolated bundle repository
    When I install the bundle
    Then both workflows and their lock are installed

  Scenario: Invalid manifests leave no installed package
    Given an isolated bundle repository
    When I install an invalid bundle
    Then no package is installed

  Scenario: Interrupted replacement is recovered by the next change
    Given an installed bundle
    When an interrupted replacement is followed by an update
    Then the interrupted replacement is recovered

  Scenario: Hand edits and unknown files prevent update and removal
    Given an installed bundle
    Then hand edits and unknown files prevent update and removal

  Scenario: An installed bundle member can be discovered and selected
    Given an installed bundle
    Then the bundle member is discoverable by its relative name
    Then the bundle member is selectable

  Scenario: A rejected update preserves the bundle and a valid update replaces it
    Given an installed bundle
    When a new revision has an invalid manifest
    Then updating fails without changing either workflow or the lock
    When the new revision is repaired and updated
    Then both workflows and their lock reflect the new revision
    And the unmodified bundle can be removed
