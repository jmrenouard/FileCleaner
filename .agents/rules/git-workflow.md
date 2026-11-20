---
trigger: always_on
description: Mandatory Git hygiene and branch isolation rules for FileCleaner (Windows Git + WSL2 Cargo isolation).
---

# Project Rule: Git Hygiene, SSH Origin & Workspace Isolation

1. **Execute Git & `gh` in Windows PowerShell**: Run all `git` and `gh` commands from Windows PowerShell in the repository root (`C:\Users\jmren\Dropbox\LightPath\Gestion2026\EnCours\FileCleaner`) to maintain uniform CRLF/LF normalization via `.gitattributes` and Windows credential/SSH key integration.
2. **SSH Remote Requirement**: Keep `origin` configured to `git@github.com:jmrenouard/FileCleaner.git`. Never push `.github/workflows/` updates over HTTPS `gho_` OAuth credentials lacking `workflow` scope.
3. **No `./target/` Inside Dropbox**: Never run `cargo` without `CARGO_TARGET_DIR="/tmp/filecleaner-target"` inside WSL2. Never commit compiler artifacts, `*.fc-tmp`, `*.pid`, `*.log`, or `coverage/` files.
4. **Dedicated Branches & Worktree Isolation**: Always branch off `main` as `<type>/issue-<number>-<slug>`. When concurrent agents or uncommitted tasks share the repository root, use `git worktree` or stage only owned files explicitly (`git add <explicit-paths>`).
5. **Atomic Conventional Commits**: Every commit message must be strictly in English using Conventional Commits (`feat:`, `fix:`, `docs:`, `ci:`, `test:`, `chore:`, `refactor:`).
