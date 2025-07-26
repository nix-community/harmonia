use std::collections::HashMap;

pub const BASE_TEMPLATE: &str = include_str!("templates/base.html");
pub const LANDING_TEMPLATE: &str = include_str!("templates/landing.html");
pub const DIRECTORY_TEMPLATE: &str = include_str!("templates/directory.html");
pub const DIRECTORY_ROW_TEMPLATE: &str = include_str!("templates/directory_row.html");

pub fn render(template: &str, variables: HashMap<&str, String>) -> String {
    let mut result = template.to_string();

    for (key, value) in variables {
        let placeholder = format!("[[{key}]]");
        result = result.replace(&placeholder, &value);
    }

    result
}

pub fn render_page(title: &str, css: &str, content: &str) -> String {
    let mut vars = HashMap::new();
    vars.insert("title", title.to_string());
    vars.insert("css", css.to_string());
    vars.insert("content", content.to_string());

    render(BASE_TEMPLATE, vars)
}
