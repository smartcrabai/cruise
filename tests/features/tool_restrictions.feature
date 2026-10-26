Feature: Prompt step restrictions enforce the tool restrictions plan
  # Production ToolPolicy computes the same policy serialized to the real gate.
  Scenario: Workflow policy restrictions cannot be relaxed by a step and duplicates collapse
    Given workflow restrictions "web,mcp:github,shell: npm  publish" and step restrictions "web,subagent,mcp:github,shell:npm publish"
    When the public policy is computed with permission full and allow_commit true
    Then workflow and step restrictions form a deduplicated union
    And MCP filtering retains "docs" without mutating the input

  Scenario: Default commit prohibition carries its own source
    Given workflow restrictions "none" and step restrictions "none"
    When the public policy is computed with permission full and allow_commit false
    Then all implicit git rules have allow_commit provenance and no explicit notice

  Scenario: Allowing commits removes only implicit rules
    Given workflow restrictions "shell:git push" and step restrictions "none"
    When the public policy is computed with permission full and allow_commit true
    Then the policy has 1 shell rules
    When the real bash gate inspects "git push origin main"
    Then the gate exits 2 with diagnostic "by disallowed_tools"

  Scenario: Explicit and implicit overlapping rules retain explicit provenance
    Given workflow restrictions "shell:git push" and step restrictions "shell:git push"
    When the public policy is computed with permission full and allow_commit false
    Then the policy has 15 shell rules
    When the real bash gate inspects "git push"
    Then the gate exits 2 with diagnostic "by disallowed_tools"

  Scenario: Allowing commits permits otherwise implicit git commands
    Given workflow restrictions "none" and step restrictions "none"
    When the public policy is computed with permission full and allow_commit true
    Then the policy has 0 shell rules
    When the real bash gate inspects "git commit -m allowed"
    Then the gate exits 0 with diagnostic "none"

  Scenario Outline: Restricted permissions discard shell rules but retain other restrictions
    Given workflow restrictions "web,shell:npm publish" and step restrictions "subagent"
    When the public policy is computed with permission <permission> and allow_commit <allow>
    Then the policy has 0 shell rules
    And web and subagent restrictions remain active
    Examples:
      | permission | allow |
      | edit       | false |
      | read-only  | false |
      | edit       | true  |
      | read-only  | true  |

  Scenario Outline: Workflow MCP filtering is subtractive
    Given workflow restrictions "<restriction>" and step restrictions "none"
    When the public policy is computed with permission full and allow_commit true
    Then MCP filtering retains "<retained>" without mutating the input
    Examples:
      | restriction | retained    |
      | mcp         | none        |
      | mcp:github  | docs        |
      | none        | github,docs |

  Scenario Outline: Wrapped and compound ref mutations are blocked by the real gate
    Given workflow restrictions "none" and step restrictions "none"
    When the public policy is computed with permission full and allow_commit false
    When the real bash gate inspects "<command>"
    Then the gate exits 2 with diagnostic "allow_commit is false"
    Examples:
      | command                                               |
      | echo ready && git commit -m x                          |
      | FOO=1 git push                                        |
      | env -u HOME KEY=value command git push                 |
      | builtin git push                                      |
      | exec git push                                         |
      | nohup git push                                        |
      | nice -n 5 git push                                    |
      | time git push                                         |
      | timeout -s TERM -k 2 5 git pull                        |
      | stdbuf -o L git push                                  |
      | xargs -r -n 1 git push                                |
      | bash -lc 'git push'                                   |
      | /usr/bin/git reset --hard                             |
      | git -C . --no-optional-locks commit -m x               |
      | git --git-dir=.git --work-tree=. push                  |
      | git -c core.hooksPath=/dev/null status                 |
      | git config core.hooksPath /dev/null                    |
      | git checkout -b topic                                 |
      | git checkout -B topic                                 |
      | echo $(git push)                                      |

  Scenario Outline: Benign commands and word-prefix nonmatches remain permitted
    Given workflow restrictions "none" and step restrictions "none"
    When the public policy is computed with permission full and allow_commit false
    When the real bash gate inspects "<command>"
    Then the gate exits 0 with diagnostic "none"
    Examples:
      | command                    |
      | git checkout -- file       |
      | git stash create           |
      | echo 'git commit'          |
      | git log --grep='git push'  |
      | git commit-tree            |
      | git status                 |
      | git branch                 |
      | git restore --staged file  |

  Scenario Outline: Invalid hook data fails closed while unrelated hooks bypass parsing
    When the real gate receives malformed case <case>
    Then the gate exits <code> with diagnostic "<diagnostic>"
    Examples:
      | case              | code | diagnostic                |
      | policy-json       | 2    | invalid shell deny policy |
      | policy-prefix     | 2    | invalid shell deny prefix |
      | input-json        | 2    | cannot read bash command  |
      | missing-command   | 2    | cannot read bash command  |
      | nonstring-command | 2    | cannot read bash command  |
      | nonbash           | 0    | none                      |
      | absent-policy     | 0    | none                      |
