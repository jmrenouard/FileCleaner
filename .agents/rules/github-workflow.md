---
trigger: always_on
description: Mandatory GitHub workflow for FileCleaner — Issue -> Dedicated Branch -> Cargo Verification -> Commit -> Push -> PR -> Merge -> Close Issue with Comment.
---

# Project Rule: Systematic GitHub Workflow (9-Step Lifecycle)

For every request made on the **FileCleaner** project, you **MUST systematically** execute all 9 steps below:

0. **Open a GitHub Issue (`gh issue create`)**: Describe the context, objectives, and acceptance criteria strictly in English and retrieve `#<id>`.
1. **Create a Dedicated Branch**: Branch from `main` using `<type>/issue-<id>-<slug>` (never commit directly to `main`; use `git worktree` when parallel edits exist in the workspace).
2. **Implement, Test & Update Governance**:
   - Run inside WSL2 Ubuntu-24.04 with `CARGO_TARGET_DIR="/tmp/filecleaner-target"`:
     - `cargo fmt --all -- --check`
     - `cargo clippy --all-targets -- -D warnings`
     - `cargo test`
   - Update `docs/ACTION_LOGBOOK.md` and `docs/RCA_REGISTER.md`.
3. **Rigorous Staging (`git add`)**: Stage only tracked project files; exclude temporary logs, `.fc-tmp`, `.pid`, or build artifacts.
4. **Structured Commit (`git commit`)**: Use Conventional Commits (`feat:`, `fix:`, `docs:`, `ci:`, `test:`, `chore:`, `refactor:`) in English referencing `Refs #<id>` or `Closes #<id>`.
5. **Branch Push (`git push`)**: Push to SSH `origin` (`git push -u origin <branch>`).
6. **Create a Pull Request (`gh pr create`)**: Target `main` with `Closes #<id>`, change summary, and verified test output strictly in English.
7. **Merge the Pull Request (`gh pr merge`)**: Merge via `gh pr merge <pr-id> --merge --delete-branch` and fast-forward local `main` (`git pull --ff-only origin main`).
8. **Close the Issue with a Summary Comment (`gh issue close --comment`)**: Record implemented changes, verification metrics, PR number, and merge commit hash strictly in English.
