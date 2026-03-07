//! Markdown → HTML conversion and file output for review results.
//!
//! Converts review markdown to a styled HTML document using comrak (GFM),
//! writes both `.md` and `.html` files to the output directory.
//! Tables are wrapped in a scrollable container via a custom formatter.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use comrak::html::ChildRendering;
use comrak::nodes::NodeValue;
use comrak::{Arena, Options, create_formatter, parse_document};

use crate::error::{Result, ReviewqError};

/// Paths to the generated review artifact files.
#[derive(Debug, Clone)]
pub struct ReviewArtifact {
    pub md_path: PathBuf,
    pub html_path: PathBuf,
}

// Custom formatter that wraps <table> elements in a scrollable div.
create_formatter!(TableWrapFormatter<()>, {
    NodeValue::Table(..) => |context, entering| {
        if entering {
            context.write_str("<div class=\"table-wrap\"><table>\n")?;
        } else {
            context.write_str("</table>\n</div>\n")?;
        }
        return Ok(ChildRendering::HTML);
    },
});

/// Convert GFM markdown to an HTML body fragment using comrak.
fn markdown_to_html_body(markdown: &str) -> String {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.header_ids = Some(String::new());
    options.render.r#unsafe = false;

    let arena = Arena::new();
    let doc = parse_document(&arena, markdown, &options);

    let mut buf = String::new();
    TableWrapFormatter::format_document(doc, &options, &mut buf, ())
        .expect("HTML formatting should not fail");
    buf
}

/// Replace label text like `[must]` in table cells with colored badge spans.
fn apply_label_badges(html: &str) -> String {
    const LABELS: &[(&str, &str)] = &[
        ("[must]", "must"),
        ("[imo]", "imo"),
        ("[ask]", "ask"),
        ("[nits]", "nits"),
        ("[suggestion]", "suggestion"),
    ];
    let mut result = html.to_string();
    for &(text, class) in LABELS {
        let badge = format!(r#"<span class="badge badge-{class}">{text}</span>"#);
        result = result.replace(text, &badge);
    }
    result
}

/// Render a full HTML document from markdown review content.
pub fn render_html(markdown: &str, title: &str) -> String {
    let body = apply_label_badges(&markdown_to_html_body(markdown));

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
<div class="layout">
<main class="container">
<div class="top-bar">
<h1 class="page-title">{title}</h1>
<button class="theme-toggle" id="theme-toggle" aria-label="Toggle theme">
<svg class="icon-sun" xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
<svg class="icon-moon" xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
</button>
</div>
{body}
</main>
<aside class="toc-sidebar" id="toc-sidebar">
<nav class="toc-nav">
<div class="toc-title">Outline</div>
<ul class="toc-list" id="toc-list"></ul>
</nav>
</aside>
</div>
<script>
(function(){{
  // Theme toggle
  var t=document.getElementById('theme-toggle'),r=document.documentElement;
  var saved=localStorage.getItem('theme');
  if(saved)r.setAttribute('data-theme',saved);
  t.addEventListener('click',function(){{
    var cur=r.getAttribute('data-theme');
    var next=cur==='light'?'dark':cur==='dark'?'light':(window.matchMedia('(prefers-color-scheme:dark)').matches?'light':'dark');
    r.setAttribute('data-theme',next);
    localStorage.setItem('theme',next);
  }});

  // TOC generation
  var headings=document.querySelectorAll('.container h1:not(.page-title), .container h2, .container h3');
  var tocList=document.getElementById('toc-list');
  var tocItems=[];
  headings.forEach(function(h){{
    var anchor=h.querySelector('a.anchor[id]');
    var hid=h.id||(anchor?anchor.id:'');
    if(!hid)return;
    var level=parseInt(h.tagName[1]);
    var li=document.createElement('li');
    li.className='toc-item toc-level-'+level;
    var a=document.createElement('a');
    a.href='#'+hid;
    a.textContent=h.textContent;
    a.addEventListener('click',function(e){{
      e.preventDefault();
      h.scrollIntoView({{behavior:'smooth',block:'start'}});
    }});
    li.appendChild(a);
    tocList.appendChild(li);
    tocItems.push({{el:h,li:li}});
  }});

  // Active heading on scroll
  function updateActive(){{
    var scrollY=window.scrollY+80;
    var active=null;
    for(var i=0;i<tocItems.length;i++){{
      if(tocItems[i].el.offsetTop<=scrollY)active=i;
    }}
    tocItems.forEach(function(item,idx){{
      item.li.classList.toggle('toc-active',idx===active);
    }});
    // Scroll active item into view in sidebar
    if(active!==null){{
      var li=tocItems[active].li;
      var nav=li.closest('.toc-nav');
      if(nav){{
        var liTop=li.offsetTop;
        var navScroll=nav.scrollTop;
        var navH=nav.clientHeight;
        if(liTop<navScroll||liTop>navScroll+navH-40){{
          nav.scrollTop=liTop-navH/3;
        }}
      }}
    }}
  }}
  window.addEventListener('scroll',updateActive,{{passive:true}});
  updateActive();
}})();
</script>
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
    --fg: #1f2328;
    --fg-muted: #656d76;
    --code-bg: #f6f8fa;
    --border: #d0d7de;
    --link: #0969da;
    --heading-border: #0969da;
    --table-stripe: #f6f8fa;
    --table-header-bg: #f0f3f6;
    --badge-must-bg: #da3633;
    --badge-must-fg: #ffffff;
    --badge-imo-bg: #bf8700;
    --badge-imo-fg: #ffffff;
    --badge-ask-bg: #0969da;
    --badge-ask-fg: #ffffff;
    --badge-nits-bg: #656d76;
    --badge-nits-fg: #ffffff;
    --badge-suggestion-bg: #1a7f37;
    --badge-suggestion-fg: #ffffff;
}

@media (prefers-color-scheme: dark) {
    :root:not([data-theme="light"]) {
        --bg: #0d1117;
        --fg: #c9d1d9;
        --fg-muted: #8b949e;
        --code-bg: #161b22;
        --border: #30363d;
        --link: #58a6ff;
        --heading-border: #1f6feb;
        --table-stripe: #131820;
        --table-header-bg: #1c2128;
        --badge-must-bg: #f85149;
        --badge-must-fg: #ffffff;
        --badge-imo-bg: #d29922;
        --badge-imo-fg: #ffffff;
        --badge-ask-bg: #58a6ff;
        --badge-ask-fg: #ffffff;
        --badge-nits-bg: #8b949e;
        --badge-nits-fg: #ffffff;
        --badge-suggestion-bg: #3fb950;
        --badge-suggestion-fg: #ffffff;
    }
}

:root[data-theme="dark"] {
    --bg: #0d1117;
    --fg: #c9d1d9;
    --fg-muted: #8b949e;
    --code-bg: #161b22;
    --border: #30363d;
    --link: #58a6ff;
    --heading-border: #1f6feb;
    --table-stripe: #131820;
    --table-header-bg: #1c2128;
    --badge-must-bg: #f85149;
    --badge-must-fg: #ffffff;
    --badge-imo-bg: #d29922;
    --badge-imo-fg: #ffffff;
    --badge-ask-bg: #58a6ff;
    --badge-ask-fg: #ffffff;
    --badge-nits-bg: #8b949e;
    --badge-nits-fg: #ffffff;
    --badge-suggestion-bg: #3fb950;
    --badge-suggestion-fg: #ffffff;
}

* { box-sizing: border-box; margin: 0; padding: 0; }

body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto,
                 "Helvetica Neue", Arial, sans-serif;
    line-height: 1.7;
    color: var(--fg);
    background: var(--bg);
    padding: 0;
}

.layout {
    display: flex;
    min-height: 100vh;
}

.container {
    flex: 1;
    min-width: 0;
    padding: 2rem 1.5rem;
}

.toc-sidebar {
    width: 260px;
    flex-shrink: 0;
    border-left: 1px solid var(--border);
    background: var(--bg);
    position: sticky;
    top: 0;
    height: 100vh;
    overflow: hidden;
}

.toc-nav {
    padding: 1.5rem 1rem;
    height: 100%;
    overflow-y: auto;
}

.toc-title {
    font-weight: 700;
    font-size: 0.85rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--fg-muted);
    margin-bottom: 0.75rem;
    padding-left: 0.5rem;
}

.toc-list {
    list-style: none;
    padding: 0;
    margin: 0;
}

.toc-item {
    margin: 0;
}

.toc-item a {
    display: block;
    padding: 0.2rem 0.5rem;
    font-size: 0.8rem;
    color: var(--fg-muted);
    text-decoration: none;
    border-left: 2px solid transparent;
    line-height: 1.4;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.toc-item a:hover {
    color: var(--fg);
    text-decoration: none;
}

.toc-item.toc-active a {
    color: var(--link);
    border-left-color: var(--link);
    font-weight: 500;
}

.toc-level-2 a { padding-left: 0.5rem; }
.toc-level-3 a { padding-left: 1.25rem; font-size: 0.78rem; }

@media (max-width: 900px) {
    .toc-sidebar { display: none; }
}

.top-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 1.5rem;
    padding-bottom: 0.5rem;
    border-bottom: 3px solid var(--heading-border);
}

.page-title {
    font-size: 1.5rem;
    font-weight: 700;
    margin: 0;
}

.theme-toggle {
    flex-shrink: 0;
    background: var(--code-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--fg);
    cursor: pointer;
    padding: 0.4rem;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background 0.2s;
}

.theme-toggle:hover { background: var(--border); }

/* Show sun in dark mode, moon in light mode */
.icon-sun { display: none; }
.icon-moon { display: block; }

@media (prefers-color-scheme: dark) {
    :root:not([data-theme="light"]) .icon-sun { display: block; }
    :root:not([data-theme="light"]) .icon-moon { display: none; }
}

:root[data-theme="dark"] .icon-sun { display: block; }
:root[data-theme="dark"] .icon-moon { display: none; }
:root[data-theme="light"] .icon-sun { display: none; }
:root[data-theme="light"] .icon-moon { display: block; }

h1, h2, h3, h4, h5, h6 {
    font-weight: 600;
    line-height: 1.3;
}

h1 { font-size: 1.6rem; border-bottom: 2px solid var(--border); padding-bottom: 0.3rem; margin-top: 2.5rem; margin-bottom: 1rem; }
h2 { font-size: 1.35rem; border-bottom: 1px solid var(--border); padding-bottom: 0.25rem; margin-top: 2.5rem; margin-bottom: 1rem; }
h3 { font-size: 1.1rem; margin-top: 2rem; margin-bottom: 0.75rem; }
h4, h5, h6 { margin-top: 1.5rem; margin-bottom: 0.5rem; }

p { margin-bottom: 1rem; }

a { color: var(--link); text-decoration: none; }
a:hover { text-decoration: underline; }

code {
    font-family: "SF Mono", "Fira Code", "Fira Mono", Menlo, Consolas, monospace;
    font-size: 0.85em;
    background: var(--code-bg);
    padding: 0.2em 0.4em;
    border-radius: 6px;
    border: 1px solid var(--border);
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
    border: none;
    font-size: 0.85em;
    line-height: 1.5;
}

blockquote {
    border-left: 4px solid var(--heading-border);
    padding: 0.5rem 1rem;
    margin: 0.8rem 0;
    background: var(--code-bg);
    border-radius: 0 6px 6px 0;
}

ul, ol {
    padding-left: 1.5rem;
    margin-bottom: 0.8rem;
}

li { margin-bottom: 0.3rem; }

.table-wrap {
    overflow-x: auto;
    margin-bottom: 1rem;
    border: 1px solid var(--border);
    border-radius: 8px;
}

table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.875em;
}

th, td {
    padding: 0.6rem 1rem;
    border: 1px solid var(--border);
    text-align: left;
    word-break: break-word;
    vertical-align: top;
    line-height: 1.6;
}

th {
    background: var(--table-header-bg);
    font-weight: 600;
    white-space: nowrap;
}

tr:nth-child(even) td { background: var(--table-stripe); }

/* Remove double border between table-wrap and table */
.table-wrap table { border: none; }
.table-wrap table th:first-child,
.table-wrap table td:first-child { border-left: none; }
.table-wrap table th:last-child,
.table-wrap table td:last-child { border-right: none; }
.table-wrap table tr:first-child th { border-top: none; }
.table-wrap table tr:last-child td { border-bottom: none; }

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
    margin: 2rem 0;
}

img { max-width: 100%; height: auto; border-radius: 4px; }

.badge {
    display: inline-block;
    font-size: 0.75em;
    font-weight: 700;
    line-height: 1;
    padding: 0.3em 0.6em;
    border-radius: 2em;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    white-space: nowrap;
}

.badge-must       { background: var(--badge-must-bg);       color: var(--badge-must-fg); }
.badge-imo        { background: var(--badge-imo-bg);        color: var(--badge-imo-fg); }
.badge-ask        { background: var(--badge-ask-bg);        color: var(--badge-ask-fg); }
.badge-nits       { background: var(--badge-nits-bg);       color: var(--badge-nits-fg); }
.badge-suggestion { background: var(--badge-suggestion-bg); color: var(--badge-suggestion-fg); }
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
        assert!(html.contains("<div class=\"table-wrap\"><table"));
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
