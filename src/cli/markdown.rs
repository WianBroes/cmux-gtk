//! Bounded Markdown preview composed over the public browser/surface APIs.
//!
//! Renders an untrusted Markdown file to a self-contained HTML page and opens
//! it in a browser surface placed right of the calling terminal, mirroring
//! `cmux diff`. The page carries no scripts and fetches no remote resources:
//! raw HTML from the Markdown is escaped and only http/https/mailto links (or
//! relative/anchor links) become clickable. Images render as their alt text so
//! no remote payload is ever requested.

use super::{browser_address, diff};
use super::socket_client::CliError;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;

/// Reject Markdown sources larger than this; the viewer page stays bounded.
const MAX_MARKDOWN_BYTES: usize = 5 * 1024 * 1024;

/// Render Markdown to an HTML fragment.
///
/// Enables tables, task lists, strikethrough and footnotes. Raw HTML embedded
/// in the Markdown is escaped, never passed through, and links with a scheme
/// other than http, https or mailto (or a relative/anchor destination) lose
/// their anchor tags while keeping their text.
pub(super) fn render(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let mut html = String::new();
    // One entry per open link/image tag: true suppresses its anchor tags.
    let mut suppressed: Vec<bool> = Vec::new();
    // Depth inside <thead>, selecting <th> over <td> for table cells.
    let mut head_depth: usize = 0;
    // Open list kinds (true is ordered) and cell kinds (true is a header).
    let mut lists: Vec<bool> = Vec::new();
    let mut cells: Vec<bool> = Vec::new();
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => html.push_str("<p>"),
                Tag::Heading { level, .. } => {
                    let _ = write!(html, "<{level}>");
                }
                Tag::BlockQuote(_) => html.push_str("<blockquote>"),
                Tag::CodeBlock(kind) => match kind {
                    CodeBlockKind::Indented => html.push_str("<pre><code>"),
                    CodeBlockKind::Fenced(info) => {
                        let language: String = info
                            .split_whitespace()
                            .next()
                            .unwrap_or_default()
                            .chars()
                            .filter(|character| {
                                character.is_ascii_alphanumeric()
                                    || matches!(character, '-' | '_' | '.')
                            })
                            .collect();
                        if language.is_empty() {
                            html.push_str("<pre><code>");
                        } else {
                            let _ = write!(
                                html,
                                "<pre><code class=\"language-{}\">",
                                escape_html(&language)
                            );
                        }
                    }
                },
                Tag::HtmlBlock => {}
                Tag::List(None) => {
                    html.push_str("<ul>");
                    lists.push(false);
                }
                Tag::List(Some(start)) => {
                    if start == 1 {
                        html.push_str("<ol>");
                    } else {
                        let _ = write!(html, "<ol start=\"{start}\">");
                    }
                    lists.push(true);
                }
                Tag::Item => html.push_str("<li>"),
                Tag::FootnoteDefinition(_) => {
                    html.push_str("<div class=\"footnote-definition\">");
                }
                Tag::Table(_) => html.push_str("<table>"),
                Tag::TableHead => {
                    html.push_str("<thead>");
                    head_depth += 1;
                }
                Tag::TableRow => html.push_str("<tr>"),
                Tag::TableCell => {
                    let header = head_depth > 0;
                    cells.push(header);
                    if header {
                        html.push_str("<th>");
                    } else {
                        html.push_str("<td>");
                    }
                }
                Tag::Emphasis => html.push_str("<em>"),
                Tag::Strong => html.push_str("<strong>"),
                Tag::Strikethrough => html.push_str("<del>"),
                Tag::Superscript => html.push_str("<sup>"),
                Tag::Subscript => html.push_str("<sub>"),
                Tag::Link { dest_url, .. } => {
                    if allowed_link(&dest_url) {
                        let _ =
                            write!(html, "<a href=\"{}\">", escape_html(&dest_url));
                        suppressed.push(false);
                    } else {
                        suppressed.push(true);
                    }
                }
                // Images render as their alt text only, so the viewer page
                // never requests a remote payload.
                Tag::Image { .. } => suppressed.push(true),
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Paragraph => html.push_str("</p>"),
                TagEnd::Heading(level) => {
                    let _ = write!(html, "</{level}>");
                }
                TagEnd::BlockQuote(_) => html.push_str("</blockquote>"),
                TagEnd::CodeBlock => html.push_str("</code></pre>"),
                TagEnd::HtmlBlock => {}
                TagEnd::List(_) => {
                    if lists.pop().unwrap_or(false) {
                        html.push_str("</ol>");
                    } else {
                        html.push_str("</ul>");
                    }
                }
                TagEnd::Item => html.push_str("</li>"),
                TagEnd::FootnoteDefinition => html.push_str("</div>"),
                TagEnd::Table => html.push_str("</table>"),
                TagEnd::TableHead => {
                    html.push_str("</thead>");
                    head_depth = head_depth.saturating_sub(1);
                }
                TagEnd::TableRow => html.push_str("</tr>"),
                TagEnd::TableCell => {
                    if cells.pop().unwrap_or(false) {
                        html.push_str("</th>");
                    } else {
                        html.push_str("</td>");
                    }
                }
                TagEnd::Emphasis => html.push_str("</em>"),
                TagEnd::Strong => html.push_str("</strong>"),
                TagEnd::Strikethrough => html.push_str("</del>"),
                TagEnd::Link => {
                    if !suppressed.pop().unwrap_or(true) {
                        html.push_str("</a>");
                    }
                }
                TagEnd::Image => {
                    suppressed.pop();
                }
                _ => {}
            },
            Event::Text(text) => html.push_str(&escape_html(&text)),
            Event::Code(code) => {
                html.push_str("<code>");
                html.push_str(&escape_html(&code));
                html.push_str("</code>");
            }
            // Raw HTML from untrusted Markdown is escaped, never passed through.
            Event::Html(content) | Event::InlineHtml(content) => {
                html.push_str(&escape_html(&content));
            }
            Event::FootnoteReference(label) => {
                let _ = write!(
                    html,
                    "<sup class=\"footnote-ref\">{}</sup>",
                    escape_html(&label)
                );
            }
            Event::SoftBreak => html.push('\n'),
            Event::HardBreak => html.push_str("<br>\n"),
            Event::Rule => html.push_str("<hr>"),
            Event::TaskListMarker(done) => {
                if done {
                    html.push_str("<input type=\"checkbox\" checked disabled> ");
                } else {
                    html.push_str("<input type=\"checkbox\" disabled> ");
                }
            }
            Event::InlineMath(content) | Event::DisplayMath(content) => {
                html.push_str(&escape_html(&content));
            }
        }
    }
    html
}

/// Whether a Markdown link destination may become a clickable anchor.
///
/// Only http, https and mailto destinations, plus relative paths and fragment
/// anchors, are allowed. Anything else (notably `javascript:`) keeps its text
/// but loses its link.
fn allowed_link(destination: &str) -> bool {
    let destination = destination.trim();
    if destination.is_empty() || destination.starts_with('#') {
        return true;
    }
    let Some(colon) = destination.find(':') else {
        // No scheme: a relative path, query or protocol-relative reference.
        return true;
    };
    let scheme = &destination[..colon];
    if scheme.is_empty()
        || scheme.contains(['/', '?', '#', ' ', '\t', '\n', '\r'])
    {
        // The colon does not separate a scheme (e.g. `path/name:x`).
        return true;
    }
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto"
    )
}

/// Escape text for an HTML body or a quoted attribute value.
fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Pick the viewer title: the first level-1 `# heading`, else the file name.
fn document_title(markdown: &str, path: &Path) -> String {
    for line in markdown.lines() {
        let stripped = line
            .trim_start_matches([' ', '\t'])
            .strip_prefix('\u{feff}')
            .unwrap_or_else(|| line.trim_start_matches([' ', '\t']));
        let Some(rest) = stripped.strip_prefix('#') else {
            continue;
        };
        // Only `# title` counts: `##` is a subheading, `#nospace` is not one.
        if rest.starts_with('#') || rest.is_empty() {
            continue;
        }
        if !(rest.starts_with(' ') || rest.starts_with('\t')) {
            continue;
        }
        let title = rest
            .trim()
            .trim_end_matches(['#', ' '])
            .trim();
        if !title.is_empty() {
            return title.chars().take(256).collect();
        }
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("cmux markdown")
        .to_owned()
}

/// Wrap a rendered fragment in a self-contained page with light and dark themes.
fn page_html(title: &str, body: &str) -> String {
    PAGE_HTML
        .replace("__CMUX_MARKDOWN_TITLE__", &escape_html(title))
        .replace("__CMUX_MARKDOWN_BODY__", body)
}

/// Read a Markdown file and write its standalone viewer page, like `diff` does.
///
/// Refuses paths that are not readable regular files, content past the 5 MiB
/// bound, and non-UTF-8 bytes, with an explicit error in each case.
pub(super) fn prepare(path: &Path) -> Result<diff::PreparedDocument, CliError> {
    let label = path.display().to_string();
    let mut file = cmux_platform::filesystem::open_regular_read(path)
        .map_err(|error| CliError::Command(format!("open {label}: {error}")))?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_MARKDOWN_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CliError::Command(format!("read markdown {label}: {error}")))?;
    if bytes.len() > MAX_MARKDOWN_BYTES {
        return Err(CliError::Command(format!(
            "markdown {label} exceeds the 5 MiB limit"
        )));
    }
    let markdown = String::from_utf8(bytes)
        .map_err(|_| CliError::Command(format!("markdown {label} is not UTF-8")))?;
    let title = document_title(&markdown, path);
    let html = page_html(&title, &render(&markdown));
    let directory = cmux_platform::paths::data_dir().join("diffs");
    cmux_platform::filesystem::create_private_directory(&directory)
        .map_err(|error| CliError::Command(format!("create markdown view directory: {error}")))?;
    diff::prune_viewers(&directory, html.len() as u64)?;
    let viewer = directory.join(format!("markdown-{}.html", uuid::Uuid::new_v4()));
    cmux_platform::filesystem::atomic_write(&viewer, html.as_bytes())
        .map_err(|error| CliError::Command(format!("write markdown viewer: {error}")))?;
    let url = browser_address::normalize(viewer.to_string_lossy().as_ref());
    let mut metadata = serde_json::Map::new();
    metadata.insert("file".into(), serde_json::Value::String(label));
    Ok(diff::PreparedDocument {
        path: viewer,
        url,
        title,
        metadata,
    })
}

const PAGE_HTML: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>__CMUX_MARKDOWN_TITLE__</title><style>
:root{color-scheme:light dark}
body{margin:0;background:#ffffff;color:#1f2328;font:16px/1.6 -apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif}
.markdown-body{max-width:50em;margin:0 auto;padding:2rem 1.5rem;overflow-wrap:break-word}
h1,h2,h3,h4,h5,h6{line-height:1.25;margin:1.5em 0 0.5em;font-weight:600}
h1{font-size:2em;border-bottom:1px solid #d0d7de;padding-bottom:0.3em}
h2{font-size:1.5em;border-bottom:1px solid #d0d7de;padding-bottom:0.3em}
a{color:#0969da;text-decoration:none}
a:hover{text-decoration:underline}
pre{background:#f6f8fa;border:1px solid #d0d7de;border-radius:6px;padding:1em;overflow:auto}
code{font:0.9em ui-monospace,SFMono-Regular,Consolas,monospace;background:#f6f8fa;border-radius:4px;padding:0.1em 0.3em}
pre code{background:none;border:0;padding:0}
table{border-collapse:collapse;width:100%;margin:1em 0;display:block;overflow:auto}
th,td{border:1px solid #d0d7de;padding:0.4em 0.8em}
th{background:#f6f8fa;font-weight:600}
tr:nth-child(even){background:#f6f8fa}
blockquote{margin:0 0 1em;padding:0 1em;border-left:0.25em solid #d0d7de;color:#57606a}
hr{border:0;border-top:1px solid #d0d7de;margin:1.5em 0}
ul,ol{padding-left:2em}
li{margin:0.25em 0}
img{max-width:100%}
.footnote-definition{font-size:0.9em;color:#57606a;border-top:1px solid #d0d7de;margin-top:1em;padding-top:0.5em}
.footnote-ref{font-size:0.8em}
input[type="checkbox"]{margin-right:0.4em}
@media (prefers-color-scheme: dark){
body{background:#0d1117;color:#e6edf3}
h1,h2{border-color:#30363d}
a{color:#4493f8}
pre{background:#161b22;border-color:#30363d}
code{background:#161b22}
th,td{border-color:#30363d}
th{background:#161b22}
tr:nth-child(even){background:#161b22}
blockquote{border-color:#30363d;color:#9198a1}
hr{border-color:#30363d}
.footnote-definition{color:#9198a1;border-color:#30363d}
}
</style></head><body><main class="markdown-body">__CMUX_MARKDOWN_BODY__</main></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

    /// A fresh scratch file path that no other test owns.
    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cmux-markdown-test-{}-{}-{name}",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Headings render as heading elements with their text.
    #[test]
    fn titles_render_as_headings() {
        let html = render("# Hello\n\n## Sub\n");
        assert!(html.contains("<h1>Hello</h1>"), "{html}");
        assert!(html.contains("<h2>Sub</h2>"), "{html}");
    }

    /// Bulleted, ordered and task lists render with checkboxes for tasks.
    #[test]
    fn lists_and_task_lists_render() {
        let html = render("- alpha\n- beta\n\n1. first\n\n- [ ] todo\n- [x] done\n");
        assert!(html.contains("<ul>"), "{html}");
        assert!(html.contains("<li>"), "{html}");
        assert!(html.contains("<ol>"), "{html}");
        assert!(html.contains("type=\"checkbox\""), "{html}");
        assert!(html.contains("checked"), "{html}");
        assert!(html.contains("todo"), "{html}");
    }

    /// Fenced code blocks and tables render with readable containers.
    #[test]
    fn code_blocks_and_tables_render() {
        let html = render(
            "```rust\nlet x = 1;\n```\n\n| name | age |\n|------|-----|\n| ann  | 3   |\n",
        );
        assert!(html.contains("<pre><code"), "{html}");
        assert!(html.contains("let x = 1;"), "{html}");
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("<th>name</th>"), "{html}");
        assert!(html.contains("<td>3</td>"), "{html}");
    }

    /// Strikethrough and footnotes are enabled in the parser options.
    #[test]
    fn strikethrough_and_footnotes_render() {
        let html = render("~~gone~~\n\nNote[^1].\n\n[^1]: the footnote.\n");
        assert!(html.contains("<del>gone</del>"), "{html}");
        assert!(html.contains("footnote-ref"), "{html}");
        assert!(html.contains("the footnote."), "{html}");
    }

    /// Raw HTML from untrusted Markdown is escaped, never passed through.
    #[test]
    fn raw_html_is_escaped() {
        let html = render("<script>alert(1)</script>\n\n<b>bold</b>\n");
        assert!(!html.contains("<script"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(html.contains("&lt;b&gt;"), "{html}");
    }

    /// Only http/https/mailto (or relative/anchor) destinations become links.
    #[test]
    fn link_schemes_are_filtered() {
        let evil = render("[evil](javascript:alert(1))\n");
        assert!(!evil.contains("<a"), "{evil}");
        assert!(evil.contains("evil"), "{evil}");
        let shouty = render("[evil](JAVASCRIPT:alert(1))\n");
        assert!(!shouty.contains("<a"), "{shouty}");
        for (markdown, expected) in [
            ("[web](https://example.com/x)", "href=\"https://example.com/x\""),
            ("[mail](mailto:a@example.com)", "href=\"mailto:a@example.com\""),
            ("[anchor](#section)", "href=\"#section\""),
            ("[page](other/page.html)", "href=\"other/page.html\""),
        ] {
            let html = render(&format!("{markdown}\n"));
            assert!(html.contains("<a "), "{html}");
            assert!(html.contains(expected), "{html}");
        }
    }

    /// The viewer title comes from the first level-1 heading.
    #[test]
    fn prepare_takes_title_from_first_heading() {
        let path = scratch("title.md");
        std::fs::write(&path, "# My Title\n\nBody.\n").unwrap();
        let prepared = prepare(&path).unwrap();
        assert_eq!(prepared.title, "My Title");
        assert!(prepared.url.starts_with("file://"), "{}", prepared.url);
        let _ = std::fs::remove_file(&prepared.path);
        let _ = std::fs::remove_file(&path);
    }

    /// Without a level-1 heading the file name becomes the title.
    #[test]
    fn prepare_falls_back_to_file_name() {
        let path = scratch("fallback-name.md");
        std::fs::write(&path, "No heading here.\n").unwrap();
        let prepared = prepare(&path).unwrap();
        assert!(prepared.title.ends_with("fallback-name.md"), "{}", prepared.title);
        let _ = std::fs::remove_file(&prepared.path);
        let _ = std::fs::remove_file(&path);
    }

    /// A level-2 heading alone does not provide the viewer title.
    #[test]
    fn prepare_ignores_subheadings_for_title() {
        let path = scratch("subheading.md");
        std::fs::write(&path, "## Only sub\n\nBody.\n").unwrap();
        let prepared = prepare(&path).unwrap();
        assert!(prepared.title.ends_with("subheading.md"), "{}", prepared.title);
        let _ = std::fs::remove_file(&prepared.path);
        let _ = std::fs::remove_file(&path);
    }

    /// A missing file is refused with an explicit error.
    #[test]
    fn prepare_rejects_missing_file() {
        let path = scratch("absent.md");
        let _ = std::fs::remove_file(&path);
        let error = prepare(&path).err().expect("missing file is refused").to_string();
        assert!(error.contains("open"), "{error}");
    }

    /// Content past the 5 MiB bound is refused with an explicit error.
    #[test]
    fn prepare_rejects_oversize_file() {
        let path = scratch("huge.md");
        let oversize = vec![b'a'; MAX_MARKDOWN_BYTES + 1];
        std::fs::write(&path, oversize).unwrap();
        let error = prepare(&path).err().expect("oversize file is refused").to_string();
        assert!(error.contains("5 MiB"), "{error}");
        let _ = std::fs::remove_file(&path);
    }

    /// Non-UTF-8 bytes are refused with an explicit error.
    #[test]
    fn prepare_rejects_non_utf8_file() {
        let path = scratch("binary.md");
        std::fs::write(&path, [0xff, 0xfe, b'a']).unwrap();
        let error = prepare(&path).err().expect("non-UTF-8 file is refused").to_string();
        assert!(error.contains("UTF-8"), "{error}");
        let _ = std::fs::remove_file(&path);
    }

    /// The standalone page carries both themes and no script tags.
    #[test]
    fn page_has_both_themes_and_no_scripts() {
        let page = page_html("Demo", &render("# Demo\n"));
        assert!(page.contains("<meta charset"), "{page}");
        assert!(page.contains("prefers-color-scheme"), "{page}");
        assert!(page.contains("@media"), "{page}");
        assert!(page.contains("50em"), "{page}");
        assert!(!page.contains("<script"), "{page}");
        assert!(!page.contains("<script src"), "{page}");
        assert!(page.contains("<title>Demo</title>"), "{page}");
    }
}
