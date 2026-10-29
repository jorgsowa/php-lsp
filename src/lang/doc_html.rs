//! Conversion of the HTML markup used in PhpStorm stub docblocks to Markdown.

/// Rewrites known HTML tags and entities as Markdown; unknown tags like `array<TKey>` stay as-is.
pub(crate) fn html_to_markdown(text: &str) -> String {
    if !text.contains(['<', '&']) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(['<', '&']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let consumed = if rest.starts_with('<') {
            tag(rest).map(|(len, replacement)| {
                out.push_str(replacement);
                len
            })
        } else {
            entity(rest).map(|(len, ch)| {
                out.push(ch);
                len
            })
        };
        let len = consumed.unwrap_or_else(|| {
            out.push(rest.as_bytes()[0] as char);
            1
        });
        rest = &rest[len..];
    }
    out.push_str(rest);
    tidy_whitespace(&out)
}

/// Parses an HTML tag at the start of `s`, returning its byte length and Markdown replacement.
fn tag(s: &str) -> Option<(usize, &'static str)> {
    let end = s.find('>')?;
    let inner = &s[1..end];
    let closing = inner.starts_with('/');
    let inner = inner.trim_start_matches('/');
    let name_len = inner
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(inner.len());
    let attrs = &inner[name_len..];
    if !(attrs.is_empty() || attrs.starts_with([' ', '/'])) {
        return None;
    }
    let replacement = match inner[..name_len].to_ascii_lowercase().as_str() {
        "br" | "p" | "ul" | "ol" | "dl" | "table" | "blockquote" => "\n\n",
        "pre" => "\n```\n",
        "li" if !closing => "\n- ",
        "dt" | "tr" if !closing => "\n",
        "b" | "strong" => "**",
        "i" | "em" => "*",
        "code" => "`",
        "td" | "th" if !closing => " ",
        "li" | "dt" | "dd" | "tr" | "td" | "th" | "tbody" | "thead" | "span" | "a" | "tt"
        | "sup" | "sub" => "",
        _ => return None,
    };
    Some((end + 1, replacement))
}

/// Decodes an HTML entity at the start of `s`, returning its byte length and character.
fn entity(s: &str) -> Option<(usize, char)> {
    let end = s.bytes().take(10).position(|b| b == b';')?;
    let name = &s[1..end];
    let ch = match name {
        "gt" => '>',
        "lt" => '<',
        "amp" => '&',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "deg" => '°',
        "pi" => 'π',
        _ => {
            let digits = name.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((end + 1, ch))
}

/// Trims each line and collapses runs of blank lines left behind by block tags.
fn tidy_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.split('\n').map(str::trim) {
        if line.is_empty() {
            blank_run += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank_run > 0 { "\n\n" } else { "\n" });
        }
        blank_run = 0;
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::html_to_markdown;

    #[test]
    fn converts_breaks_and_entities() {
        assert_eq!(
            html_to_markdown("(PHP 5 &gt;= 5.2.0)<br/> Returns the JSON"),
            "(PHP 5 >= 5.2.0)\n\nReturns the JSON"
        );
    }

    #[test]
    fn converts_inline_formatting() {
        assert_eq!(
            html_to_markdown("<p>Returns <b>FALSE</b> or <code>x</code>.</p>"),
            "Returns **FALSE** or `x`."
        );
    }

    #[test]
    fn keeps_generic_types_and_bare_ampersands() {
        let text = "array<TKey, TValue> & list<int>";
        assert_eq!(html_to_markdown(text), text);
    }

    #[test]
    fn keeps_unknown_entities() {
        assert_eq!(html_to_markdown("a &foo; b &#37;"), "a &foo; b %");
    }
}
