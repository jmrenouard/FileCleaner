---
trigger: always_on
description: Mandatory LaTeX math integrity and Markdown delimiter standards for FileCleaner documentation (EN & FR).
---

# Universal Project Rule: LaTeX Math Integrity & Markdown Delimiter Standards

This rule applies **without exception** to every Markdown file (`README.md`, `README.fr.md`, `docs/**/*.md`), specification, translation pipeline, and AI agent operating in the **FileCleaner** repository.

---

## 1. Problem Statement & Root Cause

Markdown parsers (CommonMark, GitHub Flavored Markdown / GFM) and automated translation/refactoring tools process punctuation before KaTeX/MathJax evaluates mathematical spans. This introduces silent rendering failures:

1. **Underscore / Subscript Stripping (`_`)**:
   - In Markdown, `_text_` denotes italics. Multiple underscores inside malformed math blocks or indented blocks get parsed as `<i>...</i>`, breaking subscripts like `\mathcal{F}_{\text{valid}}` or `t_{\text{now}}`.
2. **Asterisk / Superscript Stripping (`*`)**:
   - In Markdown, `*text*` denotes italics. Asterisks inside math expressions (e.g., `f^*`, `k^*`) get paired across prose lines if delimiters break.
3. **Display Math Degradation via Indentation (`$$`)**:
   - Indenting `$$` inside lists (e.g., `  $$`) or omitting surrounding blank lines causes CommonMark to treat the block as a list continuation paragraph rather than an isolated display math environment.
4. **Disallowed LaTeX Macros on GitHub (`\operatorname`)**:
   - GitHub's MathJax/KaTeX renderer does **not** permit `\operatorname{...}`. Formulas containing `\operatorname` fail to render on GitHub.

---

## 2. Mandatory Prohibition of `\operatorname` Macro

- **Strictly Prohibited**: Never use `\operatorname{...}` anywhere in Markdown math formulas.
- **GitHub-Supported Alternatives**:
  - Use `\arg\max`, `\arg\min`, `\max`, `\min`, `\sup`, `\inf`, or `\text{...}` (e.g., `\text{size}(f)`, `\text{age}(f)`, `\text{mtime}(f)`).

---

## 3. Mandatory Column-0 Display Math Standards (`$$...$$`)

Display math equations must **ALWAYS** follow the column-0 unindented block standard:
1. Every `$$` delimiter line must start at **column 0** (zero leading spaces or tabs, even when logically inside a list item).
2. Every `$$` block must be preceded and followed by an **empty blank line**.
3. Never write single-line `$$...$$` or inline display math (`... $$x$$ ...`).

### Correct (Standard Compliant — Standalone & Inside Lists)
```markdown
- **Minimum Backup Floor Invariant**:
  Let $N_{\min} = \min(|\mathcal{F}_{\text{valid}}|, \text{min\_keep\_files})$ define the protected prefix:

$$
E \cap P_{\text{floor}} = \emptyset
$$

  guaranteeing that protected recent valid backups are never evicted.
```

### Strictly Forbidden (Violations)
```markdown
<!-- WRONG: Indented inside a list -->
- **Invariant**:
  $$
  E \cap P_{\text{floor}} = \emptyset
  $$

<!-- WRONG: Single-line display math -->
$$E \cap P_{\text{floor}} = \emptyset$$

<!-- WRONG: Missing blank lines before/after $$ -->
The invariant holds:
$$
E \cap P_{\text{floor}} = \emptyset
$$
which protects backups.
```

---

## 4. Mandatory Inline Math Standards (`$...$`)

1. **Tight Delimiters (No Internal Edge Whitespace)**:
   - Correct: `$t(f_1) \ge t(f_2)$`
   - Prohibited: `$ t(f_1) \ge t(f_2) $`
2. **Escaping Literal Dollar Signs in Prose**:
   - Literal currency or shell/template variables must **NEVER** use unescaped `$`:
     - Correct: `\$100`, `` `$FILECLEANER_PATH` ``
     - Prohibited: `$100` or `$FILECLEANER_PATH` in unbackticked prose.

---

## 5. Multi-Row LaTeX Environments (`cases`, `aligned`, `matrix`)

Inside display math blocks, row breaks require double backslashes `\\`:
```markdown
$$
t(f) = \begin{cases}
\text{mtime}(f) & \text{if } \text{basis} = \text{Modified} \\
\text{birthtime}(f) & \text{if } \text{basis} = \text{Created} \\
\text{parse}(\text{name}(f)) & \text{if } \text{basis} = \text{FilenameDate}
\end{cases}
$$
```

---

## 6. Translation & Verification Protocol (`EN` <-> `FR`)

1. **Immutable Math Tokens**: Treat all `$ ... $` and `$$ ... $$` blocks as opaque immutable spans when authoring or translating between `docs/en/` and `docs/fr/`.
2. **Automated Verification**: Every Markdown documentation update must pass automated syntax verification confirming zero `\operatorname`, zero indented `$$`, zero single-line `$$...$$`, and zero unclosed/whitespace-padded inline delimiters.
