//! Repository governance, asset integrity, and Markdown LaTeX math syntax verification.

use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn collect_markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let walker = WalkDir::new(root).into_iter().filter_entry(|entry| {
        if entry.depth() == 0 {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        !(entry.file_type().is_dir()
            && matches!(
                name.as_ref(),
                ".git" | ".worktrees" | "target" | "node_modules"
            ))
    });

    for entry in walker.flatten() {
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    files
}

/// Strips inline backtick code spans (e.g. `` `code` `` or ```` ``code`` ````) from a single line
/// so prose references to `$...$` or `\operatorname` inside code spans are not misclassified as math.
fn strip_inline_code_spans(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'`' {
            let tick_start = i;
            while i < bytes.len() && bytes[i] == b'`' {
                i += 1;
            }
            let tick_len = i - tick_start;
            if let Some(close_rel) = find_closing_ticks(&bytes[i..], tick_len) {
                i += close_rel + tick_len;
                continue;
            }
            out.push_str(&line[tick_start..i]);
        } else {
            let ch = line[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn find_closing_ticks(haystack: &[u8], tick_len: usize) -> Option<usize> {
    let mut i = 0usize;
    while i < haystack.len() {
        if haystack[i] == b'`' {
            let start = i;
            while i < haystack.len() && haystack[i] == b'`' {
                i += 1;
            }
            if i - start == tick_len {
                return Some(start);
            }
        } else {
            i += 1;
        }
    }
    None
}

fn check_markdown_math_file(path: &Path, content: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut in_fence = false;
    let mut fence_char = '`';
    let mut fence_len = 0usize;
    let mut in_display_math = false;

    for (idx, raw_line) in lines.iter().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim_end_matches('\r');
        let trimmed_start = line.trim_start();

        if !in_display_math {
            if in_fence {
                let count = trimmed_start
                    .chars()
                    .take_while(|&c| c == fence_char)
                    .count();
                if count >= fence_len && trimmed_start[count..].trim().is_empty() {
                    in_fence = false;
                }
                continue;
            }
            if trimmed_start.starts_with("```") || trimmed_start.starts_with("~~~") {
                fence_char = trimmed_start.chars().next().unwrap();
                fence_len = trimmed_start
                    .chars()
                    .take_while(|&c| c == fence_char)
                    .count();
                in_fence = true;
                continue;
            }
        }

        if in_display_math {
            if line.contains("\\operatorname") {
                errors.push(format!(
                    "{}:{line_no}: forbidden \\operatorname inside display math block",
                    path.display()
                ));
            }
            if line.trim() == "$$" {
                if line != "$$" {
                    errors.push(format!(
                        "{}:{line_no}: indented closing `$$` delimiter (must start at column 0)",
                        path.display()
                    ));
                }
                if idx + 1 < lines.len() && !lines[idx + 1].trim().is_empty() {
                    errors.push(format!(
                        "{}:{line_no}: closing `$$` must be followed by a blank line",
                        path.display()
                    ));
                }
                in_display_math = false;
            } else if line.contains("$$") {
                errors.push(format!(
                    "{}:{line_no}: malformed `$$` inside display math block: {line}",
                    path.display()
                ));
            }
            continue;
        }

        let stripped = strip_inline_code_spans(line);

        if stripped.contains("\\operatorname") {
            errors.push(format!(
                "{}:{line_no}: forbidden \\operatorname outside code span",
                path.display()
            ));
        }

        if stripped.trim() == "$$" {
            if !line.starts_with("$$") || line.trim_end() != "$$" {
                errors.push(format!(
                    "{}:{line_no}: indented `$$` delimiter (must start at column 0)",
                    path.display()
                ));
            }
            if idx > 0 && !lines[idx - 1].trim().is_empty() {
                errors.push(format!(
                    "{}:{line_no}: opening `$$` must be preceded by a blank line",
                    path.display()
                ));
            }
            in_display_math = true;
            continue;
        }

        if stripped.contains("$$") {
            errors.push(format!(
                "{}:{line_no}: single-line or inline `$$` delimiter forbidden: {line}",
                path.display()
            ));
            continue;
        }

        // Validate inline `$...$` delimiters: paired, no internal leading/trailing whitespace.
        let chars: Vec<char> = stripped.chars().collect();
        let mut c_idx = 0usize;
        let mut open_pos: Option<usize> = None;

        while c_idx < chars.len() {
            if chars[c_idx] == '\\' {
                c_idx += 2;
                continue;
            }
            if chars[c_idx] == '$' {
                match open_pos {
                    None => {
                        if c_idx + 1 < chars.len() && chars[c_idx + 1].is_whitespace() {
                            errors.push(format!(
                                "{}:{line_no}: whitespace immediately after opening `$` in `{line}`",
                                path.display()
                            ));
                        }
                        open_pos = Some(c_idx);
                    }
                    Some(start) => {
                        if c_idx > start + 1 && chars[c_idx - 1].is_whitespace() {
                            errors.push(format!(
                                "{}:{line_no}: whitespace immediately before closing `$` in `{line}`",
                                path.display()
                            ));
                        }
                        open_pos = None;
                    }
                }
            }
            c_idx += 1;
        }

        if open_pos.is_some() {
            errors.push(format!(
                "{}:{line_no}: unclosed inline `$` delimiter in `{line}`",
                path.display()
            ));
        }
    }

    if in_display_math {
        errors.push(format!(
            "{}: unclosed `$$` display math block at end of file",
            path.display()
        ));
    }

    errors
}

fn validate_well_formed_xml_svg(path: &Path, xml: &str) -> std::result::Result<(), String> {
    let trimmed = xml.trim();
    if !trimmed.starts_with("<svg") && !trimmed.starts_with("<?xml") {
        return Err(format!(
            "{}: expected XML/SVG header, found start `{}`",
            path.display(),
            &trimmed[..trimmed.len().min(32)]
        ));
    }

    let bytes = trimmed.as_bytes();
    let mut stack: Vec<String> = Vec::new();
    let mut i = 0usize;
    let mut saw_root = false;

    while i < bytes.len() {
        if bytes[i] == b'<' {
            if trimmed[i..].starts_with("<!--") {
                let Some(end) = trimmed[i + 4..].find("-->") else {
                    return Err(format!("{}: unclosed XML comment", path.display()));
                };
                i += 4 + end + 3;
                continue;
            }
            if trimmed[i..].starts_with("<?") {
                let Some(end) = trimmed[i + 2..].find("?>") else {
                    return Err(format!(
                        "{}: unclosed XML processing instruction",
                        path.display()
                    ));
                };
                i += 2 + end + 2;
                continue;
            }
            let tag_body_start = i + 1;
            let mut j = tag_body_start;
            let mut quote: Option<u8> = None;
            while j < bytes.len() {
                let b = bytes[j];
                if let Some(q) = quote {
                    if b == q {
                        quote = None;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = Some(b);
                } else if b == b'>' {
                    break;
                }
                j += 1;
            }
            if j >= bytes.len() {
                return Err(format!("{}: unclosed `<` tag", path.display()));
            }

            let tag_inner = trimmed[tag_body_start..j].trim();
            if let Some(close_name) = tag_inner.strip_prefix('/') {
                let close_name = close_name.trim();
                let Some(expected) = stack.pop() else {
                    return Err(format!(
                        "{}: unexpected closing tag `</{close_name}>`",
                        path.display()
                    ));
                };
                if expected != close_name {
                    return Err(format!(
                        "{}: mismatched closing tag `</{close_name}>` (expected `</{expected}>`)",
                        path.display()
                    ));
                }
            } else {
                let self_closing = tag_inner.ends_with('/');
                let name = tag_inner
                    .trim_end_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                if name.is_empty() {
                    return Err(format!("{}: empty XML tag", path.display()));
                }
                if !saw_root {
                    if name != "svg" {
                        return Err(format!(
                            "{}: root XML element must be `<svg>`, found `<{name}>`",
                            path.display()
                        ));
                    }
                    saw_root = true;
                }
                if !self_closing {
                    stack.push(name.to_string());
                }
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }

    if !saw_root {
        return Err(format!("{}: missing `<svg>` root element", path.display()));
    }
    if !stack.is_empty() {
        return Err(format!(
            "{}: unclosed XML tags at EOF: {:?}",
            path.display(),
            stack
        ));
    }
    Ok(())
}

#[test]
fn cargo_manifest_enforces_version_1_0_0_and_pure_rust_contract() {
    let manifest_path = manifest_dir().join("Cargo.toml");
    let raw = fs::read_to_string(&manifest_path).unwrap();
    let parsed: toml::Value = toml::from_str(&raw).unwrap();

    let pkg = parsed["package"].as_table().unwrap();
    assert_eq!(pkg["name"].as_str(), Some("filecleaner"));
    assert_eq!(pkg["version"].as_str(), Some("1.0.0"));
    assert_eq!(pkg["license"].as_str(), Some("Apache-2.0"));

    let deps = parsed["dependencies"].as_table().unwrap();
    for dep_name in deps.keys() {
        assert!(
            !dep_name.ends_with("-sys"),
            "forbidden C/C++ -sys dependency detected: {dep_name}"
        );
    }
    assert!(
        parsed.get("lints").is_none(),
        "temporary `[lints.cargo] unused_dependencies` allow override must remain removed"
    );
}

#[test]
fn governance_registers_and_brand_assets_are_present_and_valid() {
    let root = manifest_dir();

    let logbook = fs::read_to_string(root.join("docs/ACTION_LOGBOOK.md")).unwrap();
    assert!(logbook.contains("Ongoing Actions"));
    assert!(logbook.contains("Completed Actions"));
    assert!(logbook.contains("ACT-001"));

    let rca = fs::read_to_string(root.join("docs/RCA_REGISTER.md")).unwrap();
    assert!(rca.contains("Errors & Bugs Encountered"));
    assert!(rca.contains("Technical Dead-Ends"));
    assert!(rca.contains("Current Approximations"));
    assert!(rca.contains("Unimplemented Ideas"));

    // Verify all SVG brand assets are well-formed XML.
    let assets_dir = root.join("docs/assets");
    let mut svg_count = 0usize;
    for entry in fs::read_dir(&assets_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("svg") {
            let xml = fs::read_to_string(&path).unwrap();
            validate_well_formed_xml_svg(&path, &xml).unwrap();
            svg_count += 1;
        }
    }
    assert!(svg_count >= 2, "expected logo.svg and header-banner.svg");

    // Verify PNG magic signature and IHDR dimensions on docs/assets/logo.png.
    let png_bytes = fs::read(assets_dir.join("logo.png")).unwrap();
    assert!(png_bytes.len() > 24);
    assert_eq!(
        &png_bytes[0..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
    );
    assert_eq!(&png_bytes[12..16], b"IHDR");
    let width = u32::from_be_bytes(png_bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png_bytes[20..24].try_into().unwrap());
    assert!(width > 0 && height > 0);
}

#[test]
fn all_repository_markdown_files_enforce_latex_math_integrity_rules() {
    let root = manifest_dir();
    let md_files = collect_markdown_files(&root);
    assert!(
        md_files
            .iter()
            .any(|p| p.ends_with(Path::new("docs/en/SPECIFICATIONS.md"))),
        "expected docs/en/SPECIFICATIONS.md"
    );
    assert!(
        md_files
            .iter()
            .any(|p| p.ends_with(Path::new("docs/fr/SPECIFICATIONS.md"))),
        "expected docs/fr/SPECIFICATIONS.md"
    );

    let mut all_errors = Vec::new();
    for path in &md_files {
        let content = fs::read_to_string(path).unwrap();
        all_errors.extend(check_markdown_math_file(path, &content));
    }

    assert!(
        all_errors.is_empty(),
        "Markdown LaTeX integrity violations:\n{}",
        all_errors.join("\n")
    );
}

#[test]
fn github_workflows_pin_actions_to_full_shas_and_avoid_run_interpolation() {
    let root = manifest_dir();
    let workflow_paths = [
        root.join(".github/workflows/ci.yml"),
        root.join(".github/workflows/release.yml"),
    ];

    for wf_path in &workflow_paths {
        let raw = fs::read_to_string(wf_path).unwrap();
        let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&raw).unwrap();
        let jobs = doc["jobs"].as_mapping().expect("workflow jobs mapping");

        for (job_key, job_val) in jobs {
            let job_name = job_key.as_str().unwrap_or("<unknown>");
            let steps = job_val["steps"].as_sequence().expect("job steps sequence");

            for (idx, step) in steps.iter().enumerate() {
                if let Some(uses_val) = step.get("uses").and_then(|v| v.as_str()) {
                    let Some((_action_ref, sha)) = uses_val.split_once('@') else {
                        panic!(
                            "{} job '{job_name}' step #{idx} uses unpinned action '{uses_val}'",
                            wf_path.display()
                        );
                    };
                    assert!(
                        sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
                        "{} job '{job_name}' step #{idx} must pin action to a 40-char hex SHA, got '{uses_val}'",
                        wf_path.display()
                    );
                }

                if let Some(run_body) = step.get("run").and_then(|v| v.as_str()) {
                    assert!(
                        !run_body.contains("${{"),
                        "{} job '{job_name}' step #{idx} interpolates expression inside run script; pass via env: instead",
                        wf_path.display()
                    );
                }
            }
        }
    }

    let release_doc: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(&workflow_paths[1]).unwrap()).unwrap();
    assert_eq!(
        release_doc["permissions"]["contents"].as_str(),
        Some("read"),
        "release.yml top-level permissions must default to contents: read"
    );
    assert_eq!(
        release_doc["jobs"]["publish-release"]["permissions"]["contents"].as_str(),
        Some("write"),
        "release.yml publish-release job must scope contents: write"
    );
}
