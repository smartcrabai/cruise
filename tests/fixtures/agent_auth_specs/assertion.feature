Feature: False authentication assertions must fail
  Scenario: Expired real CLI preflight cannot satisfy a dispatch assertion
    Given isolated expired credentials and a jcode workflow
    When the production CLI enters exec
    Then preflight passes to the observable refusing SDK boundary
