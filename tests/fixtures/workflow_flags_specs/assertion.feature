Feature: Wrong workflow flag assertions cannot pass
  Scenario: Production nonterminal execution is not allowed to pretend e2e ran
    Given an isolated planned workflow with docs enabled
    When I run the workflow without a terminal
    Then only the e2e optional effect runs
