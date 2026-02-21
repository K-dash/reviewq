//! Markdown → HTML conversion and file output for review results.
//!
//! Converts review markdown to a styled HTML document using comrak (GFM),
//! writes both `.md` and `.html` files to the output directory.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use comrak::{Options, markdown_to_html};

use crate::error::{Result, ReviewqError};

/// Paths to the generated review artifact files.
#[derive(Debug, Clone)]
pub struct ReviewArtifact {
    pub md_path: PathBuf,
    pub html_path: PathBuf,
}

/// Convert GFM markdown to an HTML body fragment using comrak.
fn markdown_to_html_body(markdown: &str) -> String {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.header_ids = Some(String::new());
    // Do not allow raw HTML passthrough.
    options.render.unsafe_ = false;

    markdown_to_html(markdown, &options)
}

/// Render a full HTML document from markdown review content.
pub fn render_html(markdown: &str, title: &str) -> String {
    let body = markdown_to_html_body(markdown);

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title}</title>
<style>
{css}
</style>
</head>
<body>
<div class="container">
<h1 class="page-title">{title}</h1>
{body}
</div>
</body>
</html>"#,
        title = html_escape(title),
        css = CSS_TEMPLATE,
        body = body,
    )
}

/// Generate the output filename stem.
///
/// Format: `{owner}_{repo_name}_pr{pr_number}_{YYYYMMDD_HHMMSS}_{head_sha_short}`
fn output_stem(
    owner: &str,
    repo_name: &str,
    pr_number: u64,
    created_at: DateTime<Utc>,
    head_sha: &str,
) -> String {
    let sha_short = &head_sha[..7.min(head_sha.len())];
    let timestamp = created_at.format("%Y%m%d_%H%M%S");
    let owner_safe = sanitize_filename(owner);
    let repo_safe = sanitize_filename(repo_name);
    format!("{owner_safe}_{repo_safe}_pr{pr_number}_{timestamp}_{sha_short}")
}

/// Write review markdown and HTML files to the output directory.
///
/// Returns [`ReviewArtifact`] with paths to both generated files.
pub fn write_review_files(
    markdown: &str,
    owner: &str,
    repo_name: &str,
    pr_number: u64,
    head_sha: &str,
    created_at: DateTime<Utc>,
    output_dir: &Path,
) -> Result<ReviewArtifact> {
    std::fs::create_dir_all(output_dir).map_err(|e| {
        ReviewqError::Process(format!(
            "failed to create output directory {}: {e}",
            output_dir.display()
        ))
    })?;

    let stem = output_stem(owner, repo_name, pr_number, created_at, head_sha);
    let title = format!("{owner}/{repo_name} PR #{pr_number}");

    // Write .md file.
    let md_path = output_dir.join(format!("{stem}.md"));
    std::fs::write(&md_path, markdown).map_err(|e| {
        ReviewqError::Process(format!(
            "failed to write markdown {}: {e}",
            md_path.display()
        ))
    })?;

    // Write .html file.
    let html = render_html(markdown, &title);
    let html_path = output_dir.join(format!("{stem}.html"));
    std::fs::write(&html_path, &html).map_err(|e| {
        ReviewqError::Process(format!("failed to write HTML {}: {e}", html_path.display()))
    })?;

    Ok(ReviewArtifact { md_path, html_path })
}

/// Replace path-unsafe characters with underscores.
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ' ' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

/// Minimal HTML escaping for attribute/title contexts.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Embedded CSS for review HTML output.
const CSS_TEMPLATE: &str = r#"
:root {
    --bg: #ffffff;
    --fg: #1a1a2e;
    --code-bg: #f5f5f7;
    --border: #e0e0e4;
    --link: #2563eb;
    --heading-border: #3b82f6;
    --table-stripe: #f9fafb;
}

@media (prefers-color-scheme: dark) {
    :root {
        --bg: #1a1a2e;
        --fg: #e0e0e4;
        --code-bg: #252540;
        --border: #3a3a5c;
        --link: #60a5fa;
        --heading-border: #3b82f6;
        --table-stripe: #1e1e36;
    }
}

* { box-sizing: border-box; margin: 0; padding: 0; }

body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto,
                 "Helvetica Neue", Arial, sans-serif;
    line-height: 1.7;
    color: var(--fg);
    background: var(--bg);
    padding: 2rem 1rem;
}

.container {
    max-width: 52rem;
    margin: 0 auto;
}

.page-title {
    font-size: 1.5rem;
    font-weight: 700;
    margin-bottom: 1.5rem;
    padding-bottom: 0.5rem;
    border-bottom: 3px solid var(--heading-border);
}

h1, h2, h3, h4, h5, h6 {
    margin-top: 1.8rem;
    margin-bottom: 0.6rem;
    font-weight: 600;
    line-height: 1.3;
}

h1 { font-size: 1.6rem; border-bottom: 2px solid var(--border); padding-bottom: 0.3rem; }
h2 { font-size: 1.3rem; }
h3 { font-size: 1.1rem; }

p { margin-bottom: 0.8rem; }

a { color: var(--link); text-decoration: none; }
a:hover { text-decoration: underline; }

code {
    font-family: "SF Mono", "Fira Code", "Fira Mono", Menlo, Consolas, monospace;
    font-size: 0.875em;
    background: var(--code-bg);
    padding: 0.15em 0.35em;
    border-radius: 4px;
}

pre {
    background: var(--code-bg);
    padding: 1rem;
    border-radius: 8px;
    overflow-x: auto;
    margin-bottom: 1rem;
    border: 1px solid var(--border);
}

pre code {
    background: none;
    padding: 0;
    font-size: 0.85em;
    line-height: 1.5;
}

blockquote {
    border-left: 4px solid var(--heading-border);
    padding: 0.5rem 1rem;
    margin: 0.8rem 0;
    background: var(--code-bg);
    border-radius: 0 4px 4px 0;
}

ul, ol {
    padding-left: 1.5rem;
    margin-bottom: 0.8rem;
}

li { margin-bottom: 0.3rem; }

table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 1rem;
    font-size: 0.9em;
}

th, td {
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--border);
    text-align: left;
}

th { background: var(--code-bg); font-weight: 600; }
tr:nth-child(even) { background: var(--table-stripe); }

ul.contains-task-list { list-style: none; padding-left: 0; }

li.task-list-item { padding-left: 1.5rem; position: relative; }

li.task-list-item input[type="checkbox"] {
    position: absolute;
    left: 0;
    top: 0.35em;
}

hr {
    border: none;
    border-top: 1px solid var(--border);
    margin: 1.5rem 0;
}

img { max-width: 100%; height: auto; border-radius: 4px; }
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    #[test]
    fn render_html_basic() {
        let html = render_html("# Hello\n\nWorld", "Test PR");
        assert!(html.contains("<h1"));
        assert!(html.contains("Hello"));
        assert!(html.contains("<p>World</p>"));
        assert!(html.contains("<title>Test PR</title>"));
    }

    #[test]
    fn render_html_gfm_extensions() {
        let md = "- [x] Done\n- [ ] TODO\n\n| A | B |\n|---|---|\n| 1 | 2 |";
        let html = render_html(md, "GFM Test");
        assert!(html.contains("<table>"));
        assert!(html.contains("checkbox"));
    }

    #[test]
    fn output_stem_format() {
        let ts = Utc.with_ymd_and_hms(2026, 2, 22, 15, 30, 45).unwrap();
        let stem = output_stem("myorg", "myrepo", 42, ts, "abc1234def");
        assert_eq!(stem, "myorg_myrepo_pr42_20260222_153045_abc1234");
    }

    #[test]
    fn write_review_files_creates_md_and_html() {
        let tmp = TempDir::new().expect("temp dir");
        let ts = Utc.with_ymd_and_hms(2026, 1, 15, 10, 0, 0).unwrap();

        let artifact = write_review_files(
            "# Review\n\nLGTM",
            "org",
            "testrepo",
            7,
            "deadbeef1234567",
            ts,
            tmp.path(),
        )
        .expect("write should succeed");

        assert!(artifact.html_path.exists());
        assert!(artifact.md_path.exists());
        assert!(artifact.html_path.to_str().unwrap().ends_with(".html"));
        assert!(artifact.md_path.to_str().unwrap().ends_with(".md"));

        let html_content = std::fs::read_to_string(&artifact.html_path).expect("read html");
        assert!(html_content.contains("Review"));
        assert!(html_content.contains("LGTM"));

        let md_content = std::fs::read_to_string(&artifact.md_path).expect("read md");
        assert_eq!(md_content, "# Review\n\nLGTM");
    }

    #[test]
    fn sanitize_filename_replaces_unsafe_chars() {
        assert_eq!(sanitize_filename("owner/repo"), "owner_repo");
        assert_eq!(sanitize_filename("a b"), "a_b");
        assert_eq!(sanitize_filename("a\\b:c"), "a_b_c");
        assert_eq!(sanitize_filename("normal"), "normal");
    }

    #[test]
    fn html_escape_special_chars() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
        assert_eq!(html_escape(r#"say "hi""#), "say &quot;hi&quot;");
    }
}
