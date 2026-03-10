I have completed the implementation described in {plan}. Review the changes with `git diff HEAD~1..HEAD` for the last commit, or `git diff $(git merge-base HEAD main)..HEAD` to see all commits on this branch relative to the base branch.

Based on the plan and the actual diff, generate Pull Request metadata in the exact format below.
Write the PR title and description in {pr.language}.

IMPORTANT: Output ONLY the block below — no preamble, explanation, or commentary.

---
title: "Write a concise PR title here"
---
Write the PR description here.
Include an overview of the changes and any relevant background.
