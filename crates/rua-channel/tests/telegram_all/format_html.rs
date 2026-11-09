use rua_channel::telegram::format_html::markdown_to_telegram_html;

#[test]
fn plain_text() {
    assert_eq!(markdown_to_telegram_html("hello world"), "hello world");
}

#[test]
fn html_entities_escaped() {
    assert_eq!(
        markdown_to_telegram_html("a < b & c > d"),
        "a &lt; b &amp; c &gt; d"
    );
}

#[test]
fn bold() {
    assert_eq!(markdown_to_telegram_html("**bold**"), "<b>bold</b>");
}

#[test]
fn italic() {
    assert_eq!(markdown_to_telegram_html("*italic*"), "<i>italic</i>");
}

#[test]
fn bold_italic() {
    assert_eq!(
        markdown_to_telegram_html("***both***"),
        "<b><i>both</i></b>"
    );
}

#[test]
fn inline_code() {
    assert_eq!(
        markdown_to_telegram_html("use `fn main()` here"),
        "use <code>fn main()</code> here"
    );
}

#[test]
fn inline_code_with_html_entities() {
    assert_eq!(
        markdown_to_telegram_html("run `a < b && c > d`"),
        "run <code>a &lt; b &amp;&amp; c &gt; d</code>"
    );
}

#[test]
fn fenced_code_block_with_lang() {
    let input = "```rust\nfn main() {\n    println!(\"<hello>\");\n}\n```";
    let expected = "<pre><code class=\"language-rust\">fn main() {\n    println!(\"&lt;hello&gt;\");\n}</code></pre>";
    assert_eq!(markdown_to_telegram_html(input), expected);
}

#[test]
fn fenced_code_block_no_lang() {
    let input = "```\nline1\nline2\n```";
    let expected = "<pre><code>line1\nline2</code></pre>";
    assert_eq!(markdown_to_telegram_html(input), expected);
}

#[test]
fn link() {
    assert_eq!(
        markdown_to_telegram_html("[click](https://example.com)"),
        "<a href=\"https://example.com\">click</a>"
    );
}

#[test]
fn heading_to_bold() {
    assert_eq!(markdown_to_telegram_html("# Title"), "<b>Title</b>");
    assert_eq!(markdown_to_telegram_html("## Sub"), "<b>Sub</b>");
    assert_eq!(markdown_to_telegram_html("### Deep"), "<b>Deep</b>");
}

#[test]
fn multiline_mixed() {
    let input = "Hello **world**\nUse `code` here\n# Heading";
    let result = markdown_to_telegram_html(input);
    assert!(result.contains("<b>world</b>"));
    assert!(result.contains("<code>code</code>"));
    assert!(result.contains("<b>Heading</b>"));
}

#[test]
fn empty_input() {
    assert_eq!(markdown_to_telegram_html(""), "");
}

#[test]
fn underscore_bold() {
    assert_eq!(markdown_to_telegram_html("__bold__"), "<b>bold</b>");
}
