I have completed the implementation described in {plan}. Review the changes in the current worktree:

- Run `git status --short` and `git diff HEAD --` to inspect uncommitted changes.
- Read any untracked files listed by `git status --short`.
- Review committed branch changes in addition to the worktree diff. Use `git diff HEAD~1..HEAD` for the latest commit, and when the branch contains multiple commits, find the repository's actual default branch and inspect the diff from its merge-base to `HEAD`; never assume a branch name.
- Do not summarize unrelated commits or files outside this worktree.

Based on the plan and the actual diff, generate Pull Request metadata in the exact format below.

Write the PR title and description in {pr.language}.

IMPORTANT: Output ONLY the block below — no preamble, explanation, or commentary.

---
title: "Write a concise PR title here"
---
Write the PR description here.
Include an overview of the changes and any relevant background.
