---
trigger: always_on
description: Mandatory FileCleaner project governance — systematic update of docs/ACTION_LOGBOOK.md and docs/RCA_REGISTER.md on every request, change, and result.
---

# Project Rule: Systematic Update of Action Logbook and RCA Register

For **every user request, every technical or documentation change, and every test or verification result** on **FileCleaner**, you **MUST MANDATORILY** update the following two living registers (strictly in English) before closing the task:

## 1. Action Logbook (`docs/ACTION_LOGBOOK.md`)

Must be updated on **every request**, **every change**, and **every result**:
- **Ongoing Actions (`IN PROGRESS`)**: Record every new request as soon as work begins (`ACT-XXX`, ISO timestamp, Issue `#<id>`, branch, **Request**).
- **Completed Actions (`COMPLETED`)**: Once implementation and tests are verified, record:
  - **Request**: Initial objective requested.
  - **Changes**: Exact created or modified files and architectural impacts.
  - **Result**: Real verification output (`cargo fmt`, `cargo clippy`, `cargo test` pass/fail counts, `cargo llvm-cov` coverage when run), PR number, and merge commit.

## 2. RCA & Technical Transparency Register (`docs/RCA_REGISTER.md`)

Must be updated on **every request**, **every change**, and **every result** across its 4 sections:
1. **Errors & Bugs Encountered (`RCA-XXX`)**: Observed Symptom, Root Cause Analysis (5 Whys), Applied Change / Fix, and Result & Non-Regression Test.
2. **Technical Dead-Ends & Abandoned Leads (`IMP-XXX`)**: Explored and rejected approaches (e.g., C `-sys` compression crates, OAuth workflow push failures) with technical rationale so no session repeats them.
3. **Current Approximations & Simplifying Assumptions (`APP-XXX`)**: Platform-specific heuristics, kernel-interface differences (`flock` + `/proc` vs Windows `share_mode(0)`), trade-offs, and removal conditions.
4. **Unimplemented Ideas & Future Roadmap (`IDEA-XXX`)**: Roadmap enhancements outside immediate release scope with priority and target module.

> **Strict Prohibition**: Never declare a request finished or merge a Pull Request without updating `docs/ACTION_LOGBOOK.md` (and `docs/RCA_REGISTER.md` whenever an error, dead-end, approximation, or roadmap idea arises).

## 3. Language & Math Integrity Compliance

- Core engineering files, source code, governance registers, and GitHub artifacts MUST remain strictly English-only ([`.agents/rules/language-policy.md`](language-policy.md)).
- User-facing documentation may exist in parallel English (`docs/en/`) and French (`docs/fr/`) editions, and all mathematical Markdown formulas MUST satisfy [`.agents/rules/latex-math-in-markdown.md`](latex-math-in-markdown.md).
