Feature: One opt-in advisor pass with fresh advice and a fail-closed workspace guard
  Scenario Outline: Preflight checks only enabled advisors on the command backend
    Given an advisor workflow on the "<backend>" backend
    When production preflight runs with advisor "<setting>"
    Then preflight reports "<result>"
    Examples:
      | backend | setting   | result   |
      | command | enabled   | rejected |
      | command | flag-off  | accepted |
      | command | user-skip | accepted |
      | jcode   | enabled   | accepted |

  Scenario Outline: Static validation enforces advisor prompt requirements
    Given a jcode workflow with advisor placement "<placement>"
    When production static validation runs
    Then static advisor validation reports "<result>"
    Examples:
      | placement      | result   |
      | top-prompt     | accepted |
      | after-pr       | rejected |
      | command-step   | rejected |
      | missing-output | rejected |
      | allow-commit   | rejected |
      | full-access    | rejected |

  Scenario Outline: The real engine only applies fresh advice and stops on fatal outcomes
    Given an advisor pass with stale advice and a "<backend-effect>" backend double
    When the production execution engine runs with advisor "<setting>"
    Then the engine reports "<result>"
    And the advisor ran "<advisor-count>" times
    And the apply step ran "<apply-count>" times with "<advice>" advice
    And the later step ran "<later-count>" times
    Examples:
      | backend-effect      | setting           | result         | advisor-count | apply-count | advice | later-count |
      | success             | enabled           | continued      | 1             | 1           | fresh  | 1           |
      | success             | skip-apply-only   | continued      | 1             | 0           | fresh  | 1           |
      | no-findings         | enabled           | continued      | 1             | 1           | none   | 1           |
      | fails               | enabled           | continued      | 1             | 0           | absent | 1           |
      | mutates             | enabled           | changed        | 1             | 0           | absent | 0           |
      | mutates-then-sleeps | enabled           | changed        | 1             | 0           | absent | 0           |
      | times-out           | enabled           | continued      | 1             | 0           | absent | 1           |
      | loses-root          | enabled           | unverifiable   | 1             | 0           | absent | 0           |
      | interrupted         | enabled           | interrupted    | 1             | 0           | absent | 0           |
      | forbidden           | flag-off          | continued      | 0             | 0           | absent | 1           |
      | forbidden           | user-skip         | continued      | 0             | 0           | absent | 1           |
      | forbidden           | skip-advisor-only | missing-advice | 0             | 0           | absent | 0           |

  Scenario Outline: Advisors use the highest available model without changing the ordinary step model
    Given an advisor pass with stale advice and a "success" backend double
    And the pass models are step "<step>", advisor "<advisor>", and plan "<plan>"
    When the production execution engine runs with advisor "enabled"
    Then the engine reports "continued"
    And the backend received advisor model "<selected>" and apply model "openai/normal"
    Examples:
      | step        | advisor        | plan        | selected       |
      | openai/step | openai/advisor | openai/plan | openai/step    |
      | absent      | openai/advisor | openai/plan | openai/advisor |
      | absent      | []             | openai/plan | openai/plan    |
      | absent      | absent         | []          | openai/normal  |
