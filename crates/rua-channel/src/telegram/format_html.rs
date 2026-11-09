pub fn markdown_to_telegram_html(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let fence_char = &trimmed[..3];
            let lang = trimmed[3..].trim();
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            if lang.is_empty() {
                result.push_str("<pre><code>");
            } else {
                result.push_str(&format!(
                    "<pre><code class=\"language-{}\">",
                    escape_html(lang)
                ));
            }
            i += 1;
            while i < lines.len() {
                let inner = lines[i];
                if inner.trim().starts_with(fence_char) {
                    break;
                }
                result.push_str(&escape_html(inner));
                result.push('\n');
                i += 1;
            }

            if result.ends_with('\n') {
                result.pop();
            }
            result.push_str("</code></pre>");
            i += 1;
            if i < lines.len() {
                result.push('\n');
            }
            continue;
        }

        if trimmed.starts_with('#') {
            let heading_text = trimmed.trim_start_matches('#').trim();
            if !heading_text.is_empty() {
                if !result.is_empty() && !result.ends_with('\n') {
                    result.push('\n');
                }
                result.push_str(&format!("<b>{}</b>", escape_html(heading_text)));
                if i + 1 < lines.len() {
                    result.push('\n');
                }
                i += 1;
                continue;
            }
        }

        let formatted = format_inline(line);
        result.push_str(&formatted);
        if i + 1 < lines.len() {
            result.push('\n');
        }
        i += 1;
    }

    result
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn format_inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len() * 2);
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '`' {
            if let Some(end) = find_closing(&chars, i + 1, '`') {
                let inner: String = chars[i + 1..end].iter().collect();
                out.push_str(&format!("<code>{}</code>", escape_html(&inner)));
                i = end + 1;
                continue;
            }
        }

        if chars[i] == '[' {
            if let Some((text, url, end)) = parse_link(&chars, i) {
                out.push_str(&format!(
                    "<a href=\"{}\">{}</a>",
                    escape_html(&url),
                    escape_html(&text)
                ));
                i = end;
                continue;
            }
        }

        if i + 2 < len
            && chars[i] == chars.get(i + 1).copied().unwrap_or('\0')
            && chars[i] == chars.get(i + 2).copied().unwrap_or('\0')
            && (chars[i] == '*' || chars[i] == '_')
        {
            let marker = chars[i];
            let marker3: String = std::iter::repeat_n(marker, 3).collect();
            if let Some(end) = find_closing_seq(&chars, i + 3, &marker3) {
                let inner: String = chars[i + 3..end].iter().collect();
                out.push_str(&format!("<b><i>{}</i></b>", escape_html(&inner)));
                i = end + 3;
                continue;
            }
        }

        if i + 1 < len && chars[i] == chars[i + 1] && (chars[i] == '*' || chars[i] == '_') {
            let marker = chars[i];
            let marker2: String = std::iter::repeat_n(marker, 2).collect();
            if let Some(end) = find_closing_seq(&chars, i + 2, &marker2) {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str(&format!("<b>{}</b>", escape_html(&inner)));
                i = end + 2;
                continue;
            }
        }

        if (chars[i] == '*' || chars[i] == '_')
            && i + 1 < len
            && chars[i + 1] != chars[i]
            && !chars[i + 1].is_whitespace()
        {
            let marker = chars[i];
            if let Some(end) = find_closing(&chars, i + 1, marker) {
                if end > i + 1 {
                    let inner: String = chars[i + 1..end].iter().collect();
                    out.push_str(&format!("<i>{}</i>", escape_html(&inner)));
                    i = end + 1;
                    continue;
                }
            }
        }

        match chars[i] {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
        i += 1;
    }

    out
}

fn find_closing(chars: &[char], start: usize, marker: char) -> Option<usize> {
    (start..chars.len()).find(|&j| chars[j] == marker)
}

fn find_closing_seq(chars: &[char], start: usize, seq: &str) -> Option<usize> {
    let seq_chars: Vec<char> = seq.chars().collect();
    let seq_len = seq_chars.len();
    if chars.len() < start + seq_len {
        return None;
    }
    (start..=chars.len() - seq_len).find(|&j| chars[j..j + seq_len] == seq_chars[..])
}

fn parse_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    if chars[start] != '[' {
        return None;
    }
    let mut j = start + 1;
    let mut depth = 1;
    while j < chars.len() && depth > 0 {
        match chars[j] {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ => {}
        }
        j += 1;
    }
    if depth != 0 {
        return None;
    }
    let text: String = chars[start + 1..j - 1].iter().collect();
    if j >= chars.len() || chars[j] != '(' {
        return None;
    }
    let url_start = j + 1;
    let mut url_end = url_start;
    depth = 1;
    while url_end < chars.len() && depth > 0 {
        match chars[url_end] {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        url_end += 1;
    }
    if depth != 0 {
        return None;
    }
    let url: String = chars[url_start..url_end - 1].iter().collect();
    Some((text, url, url_end))
}
