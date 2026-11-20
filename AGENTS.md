# FileCleaner Project Rules (AGENTS.md)

These rules apply **without exception** to every session and every agent working on the **FileCleaner** repository (`jmrenouard/FileCleaner`).

---

## 1. Mandatory 9-Step GitHub Workflow (Required for Every Request)

For **every user request** (new feature, bug fix, CI/CD, documentation, or refactoring), the agent **MUST systematically** execute the complete GitHub lifecycle below:

0. **Open a GitHub Issue (`gh issue create`)**:
   - Create a detailed issue using `gh issue create --title "<type>: <title>" --body "<context, objectives, and acceptance criteria>"` before or at the start of work.
   - Retrieve the issue number (`#<id>`).

1. **Create a Dedicated Branch**:
   - Never commit directly to `main`.
   - Create a dedicated branch from `main`: `<type>/issue-<id>-<slug>` (e.g., `docs/issue-1-governance-specs-branding`, `feat/issue-2-retention-engine`, `fix/issue-3-...`).
   - When parallel work exists in the main working directory, use `git worktree` to cleanly isolate the branch without disturbing uncommitted files from other tasks.

2. **TDD Implementation, Verification & Governance**:
   - Write or update unit and integration tests (`src/**/*.rs`, `tests/**/*.rs`).
   - Run the full verification suite inside WSL2 Ubuntu-24.04 with `CARGO_TARGET_DIR="/tmp/filecleaner-target"` (never creating `./target/` inside the Dropbox workspace):
     - `cargo fmt --all -- --check`
     - `cargo clippy --all-targets -- -D warnings`
     - `cargo test`
     - `cargo llvm-cov` (coverage reporting when measuring or gating releases)
   - Update governance registers (`docs/ACTION_LOGBOOK.md` and `docs/RCA_REGISTER.md`) before committing.

3. **Rigorous Staging (`git add`)**:
   - Stage only tracked project files via `git add`; exclude temporary files, build outputs, logs, and local caches.

4. **Structured Commit (`git commit`)**:
   - Make atomic commits following the Conventional Commits format (`feat:`, `fix:`, `ci:`, `docs:`, `test:`, `chore:`, `refactor:`) referencing the issue (`Refs #<id>` or `Closes #<id>`).

5. **Branch Push (`git push`)**:
   - Push the branch to the SSH remote repository: `git push -u origin <branch>`.

6. **Create a Pull Request (`gh pr create`)**:
   - Open a Pull Request targeting `main` via `gh pr create --base main --head <branch> --title "<type>: <title> (#<id>)" --body "Closes #<id> ..."` including a summary of changes and the verified test plan.

7. **Merge the Pull Request (`gh pr merge`)**:
   - Merge the Pull Request into `main` via `gh pr merge <pr-id> --merge --delete-branch` and synchronize the local `main` branch (`git pull --ff-only origin main`).

8. **Close Issue with Comment (`gh issue close`)**:
   - Explicitly close the corresponding GitHub Issue with a summary comment detailing the changes, test/coverage results, PR reference, and merge commit (`gh issue close <id> --comment "..."`).

---

## 2. Ongoing / Completed Action Logbook (`docs/ACTION_LOGBOOK.md`)

The agent **MUST systematically update** [`docs/ACTION_LOGBOOK.md`](docs/ACTION_LOGBOOK.md) **on every request, every change, and every result**:

1. **Upon receiving a request (`IN PROGRESS` / `Ongoing Actions`)**:
   - Add an entry in the ongoing actions table with the identifier (`ACT-XXX`), ISO timestamp, GitHub Issue reference (`#<id>`), branch name, and exact formulation of the **Request**.
2. **During and after implementation (`CHANGES & RESULT`)**:
   - Document the exact list of created/modified files (**Changes**) as well as verification metrics (**Result**: `cargo test` counts, `cargo clippy` / `cargo fmt` status, `cargo llvm-cov` percentage when measured, PR number, and merge commit).
3. **Upon completion (`COMPLETED` / `Completed Actions`)**:
   - Move the action to the completed actions section once tests and the Pull Request are validated.

---

## 3. RCA Register of Errors, Dead-Ends, Approximations, and Unimplemented Ideas (`docs/RCA_REGISTER.md`)

The agent **MUST systematically update** [`docs/RCA_REGISTER.md`](docs/RCA_REGISTER.md) **on every request, every change, and every result** whenever any of the following four events occurs:

1. **Errors & Bugs Encountered (`RCA-XXX`)**:
   - Every runtime error, test failure, clippy/build issue, cross-platform filesystem conflict, or CI/CD failure must be logged with: *Observed Symptom*, *Root Cause Analysis (5 Whys)*, *Applied Change / Fix*, and *Result / Non-Regression Test*.
2. **Technical Dead-Ends & Abandoned Leads (`IMP-XXX`)**:
   - Every explored and rejected approach (unsafe dependency, C `-sys` cross-compilation blocker, HTTPS OAuth workflow limitation, destructive deletion shortcut) must be recorded so no subsequent session repeats the same dead-end.
3. **Current Approximations & Simplifying Assumptions (`APP-XXX`)**:
   - Every platform heuristic or trade-off (e.g., Unix `flock` + Linux `/proc/*/fd` vs Windows `share_mode(0)`, sidecar/stability heuristics, trash directory conventions) must be explicitly declared with its impact and removal condition.
4. **Unimplemented Ideas & Future Roadmap (`IDEA-XXX`)**:
   - Every feature, optimization, or extension identified during implementation or review outside the immediate scope must be logged with its priority and target module.

---

## 4. Mandatory Language Policy (English Core + Bilingual EN/FR User Documentation)

All core engineering, governance, and GitHub artifacts in **FileCleaner** **MUST be written exclusively in English**, while end-user documentation is maintained in **bilingual English and French editions** ([`.agents/rules/language-policy.md`](.agents/rules/language-policy.md)):

1. **Strictly English-Only Artifacts (Zero French Permitted)**:
   - All Rust source code and tests (`src/**/*.rs`, `tests/**/*.rs`, docstrings, inline comments, identifiers, error messages, CLI help strings, and log schemas).
   - All governance files and registers (`AGENTS.md`, `.agents/rules/*.md`, `docs/ACTION_LOGBOOK.md`, `docs/RCA_REGISTER.md`, `Cargo.toml`, `.github/**`).
   - All Git/GitHub operations: commit messages, branch names, GitHub Issues (`gh issue create`), Pull Requests (`gh pr create`), PR merge messages (`gh pr merge`), and issue closing comments (`gh issue close --comment`).
2. **Bilingual User Documentation (`EN` & `FR`)**:
   - End-user specifications, guides, and READMEs are maintained in parallel English (`docs/en/*.md`, `README.md`) and French (`docs/fr/*.md`, `README.fr.md`) editions plus the bilingual landing portal (`docs/index.html`).
   - Translations must preserve exact technical contracts, CLI flags, schema keys, and immutable LaTeX math spans.

---

## 5. Mandatory LaTeX Math Integrity & Markdown Delimiter Policy

All mathematical formulas across all Markdown documentation (`README.md`, `README.fr.md`, `docs/**/*.md`) **MUST strictly comply** with [`.agents/rules/latex-math-in-markdown.md`](.agents/rules/latex-math-in-markdown.md):

1. **Unindented Column-0 Display Math Blocks (`$$`)**: Every display math equation must be placed on its own dedicated lines unindented at column 0 with empty blank lines before and after. Indentation within lists, bullet points, or continuation blocks is **strictly forbidden**. Never write single-line `$$...$$`.
2. **Disallowed LaTeX Macro Ban (`\operatorname`)**: `\operatorname{...}` is forbidden on GitHub. Use `\arg\max`, `\arg\min`, or `\text{...}`.
3. **Emphasis Collision Prevention**: Preserve subscripts (`_`) and superscript asterisks (`*`) intact; keep inline math delimiters tight (`$x$`, never `$ x $`); escape literal currency/shell dollar signs in prose (`\$100` or backtick `` `$VAR` ``).
4. **Immutability Under EN/FR Translation**: Mathematical spans (`$...$`, `$$...$$`) are immutable opaque tokens across English and French documentation.

---

## 6. Critical Rust, WSL2, Dropbox & Cross-Compilation Rules

1. **Dropbox Workspace Protection**:
   - Never run `cargo` directly against the Dropbox directory with a repo-local `target/`. Always execute Rust builds/tests inside WSL2 Ubuntu-24.04 with `CARGO_TARGET_DIR="/tmp/filecleaner-target"` so Dropbox never indexes gigabytes of compiler artifacts or triggers Windows file-lock errors.
2. **Windows Git / `gh` Execution & SSH Origin**:
   - Run all `git` and `gh` commands from Windows PowerShell in the workspace root.
   - Keep `origin` configured via SSH (`git@github.com:jmrenouard/FileCleaner.git`) so pushes updating `.github/workflows/*.yml` succeed without OAuth `workflow` scope rejections.
3. **Pure-Rust 9-Target Portability**:
   - Never introduce C/C++ `-sys` crates (e.g., `zstd-sys`, `liblz4-sys`, `openssl-sys`) without explicit approval and cross-compiler CI verification. Prefer pure-Rust crates (`flate2` with default `miniz_oxide`, `lz4_flex`, `brotli`, `zip` without C features) so all 9 release targets compile deterministically.
