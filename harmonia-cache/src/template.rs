use std::collections::HashMap;

use askama_escape::{Html, escape};

pub const BASE_TEMPLATE: &str = include_str!("templates/base.html");
pub const LANDING_TEMPLATE: &str = include_str!("templates/landing.html");
pub const LANDING_WITH_KEYS_TEMPLATE: &str = include_str!("templates/landing_with_keys.html");
pub const DIRECTORY_TEMPLATE: &str = include_str!("templates/directory.html");
pub const DIRECTORY_ROW_TEMPLATE: &str = include_str!("templates/directory_row.html");

/// HTML-escape for element content and double-quoted attributes. Callers must
/// escape untrusted values themselves; [`render`] does not, since some slots
/// take pre-rendered HTML.
pub fn html_escape(value: &str) -> String {
    escape(value, Html).to_string()
}

/// Substitute `[[key]]` placeholders. Single pass over the template; inserted
/// values are never re-scanned, so attacker-controlled strings cannot hijack
/// other slots.
pub fn render(template: &str, variables: HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("[[") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        if let Some(close) = after.find("]]")
            && let Some(value) = variables.get(&after[..close])
        {
            out.push_str(value);
            rest = &after[close + 2..];
        } else {
            out.push_str("[[");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

pub fn render_page(title: &str, css: &str, content: &str) -> String {
    let mut vars = HashMap::new();
    vars.insert("title", title.to_string());
    vars.insert("css", css.to_string());
    vars.insert("content", content.to_string());

    render(BASE_TEMPLATE, vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_is_single_pass() {
        let mut vars = HashMap::new();
        vars.insert("a", "[[b]]".to_string());
        vars.insert("b", "PWNED".to_string());
        // Known key: value inserted verbatim, not re-expanded.
        // Unknown key: left as-is.
        assert_eq!(render("[[a]] [[nope]]", vars), "[[b]] [[nope]]");
    }
}
