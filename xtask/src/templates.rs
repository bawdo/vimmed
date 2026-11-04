//! Packaging templates: `{{key}}` placeholders filled from a `Names`, so the
//! files under `packaging/templates/` never spell the project name.
//!
//! Keys: `slug`, `kebab`, `bundle_name`, `bundle_id`, `display_name`,
//! `version`, `libexec` (the Linux directory holding the window binary),
//! `license` and `maintainer` (the two constants below).

use app_meta::derive::Names;
use std::path::Path;

/// The licence every package declares (the owner's decision), as an SPDX
/// identifier. The text is in `LICENSE` at the workspace root.
pub const LICENSE: &str = "MIT";
/// The packages' maintainer and author, set here and nowhere else.
pub const MAINTAINER: &str = "Keith Bawden <keith@bawdo.com>";

/// Replaces every `{{key}}` with its value from `n`. An unknown key is an
/// error, so a typo can't ship as literal braces.
pub fn render(src: &str, n: &Names) -> Result<String, String> {
    let vars = [
        ("slug", n.slug.as_str()),
        ("kebab", n.kebab.as_str()),
        ("bundle_name", n.bundle_name.as_str()),
        ("bundle_id", n.bundle_id.as_str()),
        ("display_name", n.display_name.as_str()),
        ("version", app_meta::VERSION),
        ("libexec", n.linux_libexec_dir.as_str()),
        ("license", LICENSE),
        ("maintainer", MAINTAINER),
    ];
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
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
            .ok_or_else(|| format!("unknown template placeholder `{key}`"))?;
        out.push_str(value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The names this build was compiled with, from the `app_meta` constants.
pub fn current_names() -> Names {
    Names {
        slug: app_meta::SLUG.into(),
        kebab: app_meta::KEBAB.into(),
        display_name: app_meta::DISPLAY_NAME.into(),
        bundle_name: app_meta::BUNDLE_NAME.into(),
        bundle_id: app_meta::BUNDLE_ID.into(),
        env_prefix: app_meta::ENV_PREFIX.into(),
        window_bin: app_meta::WINDOW_BIN.into(),
        linux_libexec_dir: app_meta::LINUX_LIBEXEC_DIR.into(),
    }
}

/// Reads `packaging/templates/<file>` under `root` and renders it.
pub fn render_file(root: &Path, file: &str, n: &Names) -> Result<String, String> {
    let path = root.join("packaging/templates").join(file);
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    render(&text, n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_meta::derive::Names;

    fn names() -> Names {
        Names::from_slug("abc_def", "com.bawdo").unwrap()
    }

    fn template(file: &str) -> String {
        let path = format!(
            "{}/../packaging/templates/{file}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn render_replaces_all_placeholders() {
        let src = "{{slug}}|{{kebab}}|{{bundle_name}}|{{bundle_id}}|{{display_name}}|{{version}}|{{libexec}}|{{license}}|{{maintainer}}";
        let got = render(src, &names()).unwrap();
        assert_eq!(
            got,
            format!(
                "abc_def|abc-def|AbcDef|com.bawdo.abc-def|abc_def|{}|/usr/lib/abc_def|MIT|Keith Bawden <keith@bawdo.com>",
                app_meta::VERSION
            )
        );
    }

    #[test]
    fn license_and_maintainer_come_from_one_place() {
        assert_eq!(LICENSE, "MIT");
        assert_eq!(MAINTAINER, "Keith Bawden <keith@bawdo.com>");
        for file in [
            "pkg-Cargo.toml.in",
            "PKGBUILD.in",
            "metainfo.xml.in",
            "app.desktop.in",
            "Info.plist.in",
        ] {
            let text = template(file);
            assert!(!text.contains("LicenseRef"), "{file}");
            assert!(!text.contains("Keith"), "{file} spells the maintainer");
            assert!(!text.contains("MIT"), "{file} spells the licence");
        }
        let metainfo = render(&template("metainfo.xml.in"), &names()).unwrap();
        assert!(metainfo.contains("<project_license>MIT</project_license>"));
    }

    #[test]
    fn unknown_placeholder_errors() {
        assert!(render("{{nope}}", &names()).is_err());
        assert!(render("{{SLUG}}", &names()).is_err());
        assert!(render("{{slug", &names()).is_err());
        assert_eq!(render("plain", &names()).unwrap(), "plain");
    }

    #[test]
    fn desktop_exec_uses_background_and_single_file() {
        let desktop = render(&template("app.desktop.in"), &names()).unwrap();
        let lines: Vec<&str> = desktop.lines().collect();
        assert!(lines.contains(&"Exec=abc_def -b %f"));
        assert!(lines.contains(&"MimeType=text/markdown;text/x-markdown;"));
        assert!(lines.contains(&"Terminal=false"));
        assert!(lines.contains(&"Type=Application"));
    }

    #[test]
    fn info_plist_template_renders() {
        let plist = render(&template("Info.plist.in"), &names()).unwrap();
        assert!(plist.contains("<string>com.bawdo.abc-def</string>"));
        assert!(plist.contains("<string>AbcDef</string>"));
        assert!(plist.contains("<string>14.0</string>"));
        assert!(plist.contains("net.daringfireball.markdown"));
        assert!(plist.contains("public.plain-text"));
        assert!(!plist.contains("{{"));
    }

    #[test]
    fn every_template_renders_without_leftovers() {
        for file in [
            "Info.plist.in",
            "app.desktop.in",
            "metainfo.xml.in",
            "PKGBUILD.in",
            "pkg-Cargo.toml.in",
        ] {
            let text = render(&template(file), &names()).unwrap();
            assert!(!text.contains("{{"), "{file} kept a placeholder");
            assert!(!text.contains("}}"), "{file} kept a placeholder");
        }
    }

    #[test]
    fn current_names_match_app_meta() {
        let n = current_names();
        assert_eq!(n.slug, app_meta::SLUG);
        assert_eq!(n.bundle_id, app_meta::BUNDLE_ID);
        assert_eq!(n.linux_libexec_dir, app_meta::LINUX_LIBEXEC_DIR);
    }
}
