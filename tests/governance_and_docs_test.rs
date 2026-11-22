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
fn cargo_manifest_enforces_version_and_pure_rust_contract() {
    let manifest_path = manifest_dir().join("Cargo.toml");
    let raw = fs::read_to_string(&manifest_path).unwrap();
    let parsed: toml::Value = toml::from_str(&raw).unwrap();

    let pkg = parsed["package"].as_table().unwrap();
    assert_eq!(pkg["name"].as_str(), Some("filecleaner"));
    assert_eq!(pkg["version"].as_str(), Some("1.0.1"));
    assert_eq!(pkg["license"].as_str(), Some("Apache-2.0"));

    let deps = parsed["dependencies"].as_table().unwrap();
    for dep_name in deps.keys() {
        assert!(
            !dep_name.ends_with("-sys")
                && dep_name != "ring"
                && !dep_name.starts_with("aws-lc")
                && !dep_name.starts_with("openssl"),
            "forbidden C/C++ or native crypto dependency detected in Cargo.toml: {dep_name}"
        );
    }
    let ureq_dep = deps["ureq"].as_table().expect("ureq must be a table dep");
    assert_eq!(
        ureq_dep.get("default-features").and_then(|v| v.as_bool()),
        Some(false)
    );
    let rustls_dep = deps["rustls"]
        .as_table()
        .expect("rustls must be a table dep");
    assert_eq!(
        rustls_dep.get("default-features").and_then(|v| v.as_bool()),
        Some(false)
    );

    // Verify Cargo.lock contains zero forbidden C/C++ library bindings and keeps rustls/ureq off `ring`.
    let lock_raw = fs::read_to_string(manifest_dir().join("Cargo.lock")).unwrap();
    let lock_val: toml::Value = toml::from_str(&lock_raw).unwrap();
    let packages = lock_val["package"].as_array().expect("Cargo.lock packages");
    let forbidden_exact = [
        "openssl",
        "openssl-sys",
        "aws-lc-rs",
        "aws-lc-sys",
        "aws-lc-fips-sys",
        "zstd-sys",
        "liblz4-sys",
        "bzip2-sys",
        "zlib-ng-sys",
        "libz-sys",
        "curl-sys",
        "cmake",
    ];
    for pkg_entry in packages {
        let name = pkg_entry["name"].as_str().unwrap_or("");
        assert!(
            !forbidden_exact.contains(&name),
            "forbidden native/C dependency detected in Cargo.lock: {name}"
        );
        if matches!(
            name,
            "filecleaner" | "ureq" | "rustls" | "rustls-rustcrypto"
        ) {
            if let Some(pkg_deps) = pkg_entry.get("dependencies").and_then(|d| d.as_array()) {
                for dep in pkg_deps {
                    let dep_str = dep.as_str().unwrap_or("");
                    assert!(
                        !dep_str.starts_with("ring")
                            && !dep_str.starts_with("aws-lc")
                            && !dep_str.starts_with("native-tls"),
                        "package '{name}' in Cargo.lock must not depend on native crypto '{dep_str}'"
                    );
                }
            }
        }
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
        root.join(".github/workflows/docker-publish.yml"),
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

                if let Some(if_val) = step.get("if").and_then(|v| v.as_str()) {
                    assert!(
                        !if_val.contains("secrets."),
                        "{} job '{job_name}' step #{idx} must not reference secrets directly inside if:",
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

    let docker_raw = fs::read_to_string(&workflow_paths[2]).unwrap();
    let docker_doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&docker_raw).unwrap();
    assert_eq!(
        docker_doc["permissions"]["contents"].as_str(),
        Some("read"),
        "docker-publish.yml must default to contents: read"
    );
    let trigger_block = docker_raw.split("jobs:").next().unwrap_or("");
    assert!(trigger_block.contains("types: [published]"));
    assert!(trigger_block.contains("workflow_dispatch:"));
    assert!(!trigger_block.contains("push:"));
    assert!(!trigger_block.contains("tags:"));
    assert!(docker_raw.contains("jmrenouard/filecleaner"));
    assert!(docker_raw.contains("linux/amd64,linux/arm64"));
}

#[test]
fn docker_packaging_enforces_security_and_multistage_standards() {
    let root = manifest_dir();
    let dockerfile = fs::read_to_string(root.join("Dockerfile")).expect("Dockerfile must exist");
    let dockerignore =
        fs::read_to_string(root.join(".dockerignore")).expect(".dockerignore must exist");

    assert!(dockerfile.contains("FROM --platform=$BUILDPLATFORM rust:1.100.0-bookworm AS builder"));
    assert!(dockerfile.contains("FROM debian:bookworm-slim AS runtime"));
    assert!(!dockerfile.contains(":latest"));
    assert!(dockerfile.contains("groupadd -g 10001 fcgroup"));
    assert!(dockerfile.contains("useradd -u 10001 -g fcgroup"));
    assert!(dockerfile.contains("USER fcuser"));
    assert!(dockerfile.contains("HEALTHCHECK"));
    assert!(dockerfile.contains("ENTRYPOINT [\"/usr/local/bin/filecleaner\"]"));
    assert!(dockerfile.contains("VOLUME"));
    assert!(dockerfile.contains("Apache-2.0"));

    for pat in [".git", "target", "tests", ".agents", "dist", "coverage"] {
        assert!(
            dockerignore.lines().any(|l| l.trim() == pat),
            ".dockerignore must exclude '{pat}'"
        );
    }
}

fn heading_signature_outside_fences(markdown: &str) -> Vec<usize> {
    let mut levels = Vec::new();
    let mut in_fence = false;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && line.chars().nth(hashes) == Some(' ') {
            levels.push(hashes);
        }
    }
    levels
}

fn extract_fenced_config_blocks(markdown: &str) -> Vec<(&'static str, usize, String)> {
    let mut blocks = Vec::new();
    let mut current_lang: Option<&'static str> = None;
    let mut start_line = 0usize;
    let mut buf = String::new();

    for (idx, line) in markdown.lines().enumerate() {
        let trimmed = line.trim();
        if current_lang.is_none() {
            if trimmed == "```toml" {
                current_lang = Some("toml");
                start_line = idx + 1;
                buf.clear();
            } else if trimmed == "```yaml" || trimmed == "```yml" {
                current_lang = Some("yaml");
                start_line = idx + 1;
                buf.clear();
            }
        } else if trimmed.starts_with("```") {
            let lang = current_lang.take().unwrap();
            let has_rules = match lang {
                "toml" => buf.contains("[[rules]]"),
                "yaml" => buf.lines().any(|l| l.trim_start() == "rules:"),
                _ => false,
            };
            if has_rules {
                blocks.push((lang, start_line, buf.clone()));
            }
            buf.clear();
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }

    blocks
}

#[test]
fn howto_en_and_fr_editions_exist_and_keep_heading_parity() {
    let root = manifest_dir();
    let en_path = root.join("docs/en/HOWTO.md");
    let fr_path = root.join("docs/fr/HOWTO.md");

    let en = fs::read_to_string(&en_path).expect("docs/en/HOWTO.md must exist");
    let fr = fs::read_to_string(&fr_path).expect("docs/fr/HOWTO.md must exist");

    assert!(en.contains("../fr/HOWTO.md"));
    assert!(en.contains("USER_GUIDE.md"));
    assert!(fr.contains("../en/HOWTO.md"));
    assert!(fr.contains("GUIDE_UTILISATEUR.md"));

    assert_eq!(
        heading_signature_outside_fences(&en),
        heading_signature_outside_fences(&fr),
        "docs/en/HOWTO.md and docs/fr/HOWTO.md must maintain identical heading structure"
    );
}

#[test]
fn all_documented_configs_and_examples_parse_and_validate() {
    let root = manifest_dir();
    let doc_paths = [
        "README.md",
        "README.fr.md",
        "docs/en/HOWTO.md",
        "docs/fr/HOWTO.md",
        "docs/en/COOKBOOK_EXAMPLES.md",
        "docs/fr/EXEMPLES_RECETTES.md",
    ];

    let mut validated_blocks = 0usize;
    for rel in doc_paths {
        let path = root.join(rel);
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        for (lang, start_line, block) in extract_fenced_config_blocks(&content) {
            let cfg: filecleaner::Config = match lang {
                "toml" => toml::from_str(&block).unwrap_or_else(|e| {
                    panic!("{rel}:{start_line} invalid TOML config block: {e}\n{block}")
                }),
                "yaml" => serde_yaml_ng::from_str(&block).unwrap_or_else(|e| {
                    panic!("{rel}:{start_line} invalid YAML config block: {e}\n{block}")
                }),
                _ => unreachable!(),
            };
            cfg.validate().unwrap_or_else(|e| {
                panic!("{rel}:{start_line} config validation failed: {e}\n{block}")
            });
            validated_blocks += 1;
        }
    }
    assert!(
        validated_blocks >= 10,
        "expected >= 10 validated doc config blocks, found {validated_blocks}"
    );

    let examples_dir = root.join("examples");
    let mut validated_examples = 0usize;
    for entry in fs::read_dir(&examples_dir).expect("examples/ directory must exist") {
        let path = entry.unwrap().path();
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if matches!(ext, "toml" | "yaml" | "yml") {
            filecleaner::Config::load_validated(&path).unwrap_or_else(|e| {
                panic!("example config {} failed validation: {e}", path.display())
            });
            validated_examples += 1;
        }
    }
    assert!(
        validated_examples >= 1,
        "expected at least one validated config in examples/"
    );
}
