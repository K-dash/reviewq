//! Regenerate HTML from an existing markdown review file.
//!
//! Usage: cargo run --example regen_html -- <md_path> <title>

use reviewq::review_html::render_html;

fn main() {
    let md_path = std::env::args()
        .nth(1)
        .expect("usage: regen_html <md_path> <title>");
    let title = std::env::args()
        .nth(2)
        .expect("usage: regen_html <md_path> <title>");

    let markdown = std::fs::read_to_string(&md_path).expect("failed to read markdown file");
    let html = render_html(&markdown, &title);

    let html_path = md_path.replace(".md", ".html");
    std::fs::write(&html_path, &html).expect("failed to write HTML file");
    eprintln!("Wrote {html_path}");
}
