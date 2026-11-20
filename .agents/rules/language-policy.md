---
trigger: always_on
description: Mandatory FileCleaner language policy — strictly English code, comments, governance, and GitHub operations; bilingual EN/FR end-user documentation.
---

# Project Rule: Mandatory Language Policy (English Core + Bilingual EN/FR User Docs)

All engineering, governance, and GitHub artifacts in the **FileCleaner** repository **MUST be written exclusively in English**, while user-facing specifications, guides, and portals are maintained in synchronized **English and French** editions:

1. **Strictly English-Only Artifacts (Zero Non-English Text Permitted)**:
   - **Source Code & Tests**: All Rust files (`src/**/*.rs`, `tests/**/*.rs`, `benches/**/*.rs`), doc comments (`///`, `//!`), inline comments (`//`), identifiers, `thiserror` error messages, CLI `--help` descriptions, and structured JSONL log fields.
   - **Project Governance & Configuration**: `AGENTS.md`, `.agents/rules/*.md`, `docs/ACTION_LOGBOOK.md`, `docs/RCA_REGISTER.md`, `Cargo.toml`, `.github/**/*.yml`, `.gitignore`, `.gitattributes`, and `LICENSE`.
   - **Git & GitHub Artifacts**: All commit messages, branch names, GitHub Issues (`gh issue create`), Pull Requests (`gh pr create`), PR merge commits (`gh pr merge`), review comments, and issue closing comments (`gh issue close --comment`).

2. **Bilingual End-User Documentation (`EN` & `FR`)**:
   - **English Edition**: `README.md`, `docs/en/**/*.md` (e.g., `docs/en/SPECIFICATIONS.md`, user guides, cookbooks).
   - **French Edition**: `README.fr.md`, `docs/fr/**/*.md` (e.g., `docs/fr/SPECIFICATIONS.md`, user guides, cookbooks).
   - **Web Showcase**: `docs/index.html` (bilingual EN/FR UI support).
   - **Synchronization Rule**: Any CLI flag, configuration field, safety invariant, or mathematical formula updated in `docs/en/` MUST be reflected identically in `docs/fr/` with immutable LaTeX math spans and identical schema examples.
