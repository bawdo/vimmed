//! Packaging templates: `{{KEY}}` placeholders filled from `app_meta`, so
//! the files under `packaging/templates/` never spell the project name.

/// Replaces every `{{KEY}}` with its value. A placeholder without a value
/// is an error, so a typo can't ship as literal braces.
pub fn render(template: &str, vars: &[(&str, &str)]) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| "unclosed `{{` in template".to_string())?;
        let key = &after[..end];
        let value = vars
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
            .ok_or_else(|| format!("template placeholder `{key}` has no value"))?;
        out.push_str(value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The values every template may use.
pub fn app_vars() -> Vec<(&'static str, &'static str)> {
    vec![
        ("SLUG", app_meta::SLUG),
        ("DISPLAY_NAME", app_meta::DISPLAY_NAME),
        ("BUNDLE_NAME", app_meta::BUNDLE_NAME),
        ("BUNDLE_ID", app_meta::BUNDLE_ID),
        ("VERSION", app_meta::VERSION),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_placeholders() {
        let got = render("<a>{{X}}</a><b>{{Y}}{{X}}</b>", &[("X", "1"), ("Y", "2")]);
        assert_eq!(got.unwrap(), "<a>1</a><b>21</b>");
    }

    #[test]
    fn unknown_or_unclosed_placeholders_fail() {
        assert!(render("{{NOPE}}", &[]).is_err());
        assert!(render("{{X", &[("X", "1")]).is_err());
        assert_eq!(render("plain", &[]).unwrap(), "plain");
    }

    #[test]
    fn info_plist_template_renders() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../packaging/templates/Info.plist.in"
        );
        let text = std::fs::read_to_string(path).unwrap();
        let plist = render(&text, &app_vars()).unwrap();
        assert!(plist.contains(&format!("<string>{}</string>", app_meta::BUNDLE_ID)));
        assert!(plist.contains("<string>14.0</string>"));
        assert!(plist.contains("net.daringfireball.markdown"));
        assert!(plist.contains("public.plain-text"));
        assert!(!plist.contains("{{"));
    }
}
