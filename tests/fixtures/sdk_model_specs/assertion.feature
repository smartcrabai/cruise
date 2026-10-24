Feature: Wrong SDK dispatch assertions cannot silently pass
  Scenario: An intentionally wrong order fails after real successful Claude execution
    Given an isolated workflow with a single successful Claude candidate
    When the real CLI runs exec against refusing local SDK processes
    Then SDK subprocesses ran in order jcode claude jcode
