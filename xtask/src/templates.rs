//! Packaging templates: `{{key}}` placeholders filled from a `Names`, so the
//! files under `packaging/templates/` never spell the project name.
//!
//! Keys: `slug`, `kebab`, `bundle_name`, `bundle_id`, `display_name`,
//! `version`, `libexec` (the Linux directory holding the window binary),
//! `license` and `maintainer` (the two constants below), `maintainer_name`
//! (the maintainer without the email), `developer_id` (the bundle id's
//! reverse-DNS prefix) and `homepage` (the workspace `repository` URL).
//!
//! `release_date` and `release_type` need the repository (CHANGELOG.md and
//! git), so only `render_file` and `render_with` supply them.

use app_meta::derive::Names;
use std::path::Path;

/// The licence every package declares (the owner's decision), as an SPDX
/// identifier. The text is in `LICENSE` at the workspace root.
pub const LICENSE: &str = "MIT";
/// The packages' maintainer and author, set here and nowhere else.
pub const MAINTAINER: &str = "Keith Bawden <keith@bawdo.com>";

/// The AppStream release facts: a `YYYY-MM-DD` date and `stable` or
/// `development`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub date: String,
    pub kind: &'static str,
}

/// The maintainer's name without the `<email>`.
pub fn maintainer_name() -> &'static str {
    MAINTAINER
        .split_once('<')
        .map_or(MAINTAINER, |(name, _)| name)
        .trim()
}

/// The reverse-DNS prefix of the bundle id (`com.example` of
/// `com.example.app`), which is the id prefix from the workspace manifest.
fn developer_id(n: &Names) -> &str {
    n.bundle_id
        .strip_suffix(n.kebab.as_str())
        .and_then(|p| p.strip_suffix('.'))
        .unwrap_or(&n.bundle_id)
}

/// True for `YYYY-MM-DD` with a plausible month and day.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(0);
    (1..=12).contains(&num(5..7)) && (1..=31).contains(&num(8..10))
}

/// Finds the date of `version`'s heading, `## [X.Y.Z] - YYYY-MM-DD` (the
/// format `scripts/changelog-version.sh` checks), in a CHANGELOG. `Ok(None)`
/// means no heading for exactly that version (only `[Unreleased]`, or other
/// versions). Any other `## [` heading is malformed and an error, as in the
/// script.
pub fn changelog_release_date(changelog: &str, version: &str) -> Result<Option<String>, String> {
    let mut found = None;
    for line in changelog.lines().filter(|l| l.starts_with("## [")) {
        if line == "## [Unreleased]" {
            continue;
        }
        let parsed = line
            .strip_prefix("## [")
            .and_then(|r| r.split_once("] - "))
            .filter(|(v, d)| {
                let parts: Vec<&str> = v.split('.').collect();
                parts.len() == 3
                    && parts
                        .iter()
                        .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
                    && is_date(d)
            });
        let Some((v, d)) = parsed else {
            return Err(format!(
                "malformed release heading '{line}' (want '## [X.Y.Z] - YYYY-MM-DD')"
            ));
        };
        if v == version && found.is_none() {
            found = Some(d.to_string());
        }
    }
    Ok(found)
}

/// Picks the release: the CHANGELOG date as `stable` when it has one for
/// `version`, otherwise `development` dated by `commit_date`.
pub fn pick_release(
    changelog: &str,
    version: &str,
    commit_date: impl FnOnce() -> Result<String, String>,
) -> Result<Release, String> {
    match changelog_release_date(changelog, version)? {
        Some(date) => Ok(Release {
            date,
            kind: "stable",
        }),
        None => {
            let date = commit_date()?;
            if !is_date(&date) {
                return Err(format!("git gave '{date}', not a YYYY-MM-DD date"));
            }
            Ok(Release {
                date,
                kind: "development",
            })
        }
    }
}

/// The HEAD commit date (`YYYY-MM-DD`). `safe.directory` is set on the call
/// so it works in the check container, where root reads a checkout owned by
/// the host user, whichever way the caller started.
fn head_commit_date(root: &Path) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-c")
        .arg(format!("safe.directory={}", root.display()))
        .arg("-C")
        .arg(root)
        .args(["log", "-1", "--format=%cs"])
        .output()
        .map_err(|e| format!("run git for the release date: {e}"))?;
    let date = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || date.is_empty() {
        return Err(format!(
            "git could not give the HEAD commit date for the release (is {} a git checkout with a commit?): {}",
            root.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(date)
}

/// The release for the current version, from CHANGELOG.md and git under `root`.
pub fn current_release(root: &Path) -> Result<Release, String> {
    let path = root.join("CHANGELOG.md");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    pick_release(&text, app_meta::VERSION, || head_commit_date(root))
}

/// Replaces every `{{key}}` with its value from `n`. An unknown key is an
/// error, so a typo can't ship as literal braces. The `release_*` keys are
/// errors here: use `render_with`. Only the tests need the plain form.
#[cfg(test)]
fn render(src: &str, n: &Names) -> Result<String, String> {
    render_with(src, n, None)
}

/// As `render`, also filling `release_date` and `release_type` from `release`.
pub fn render_with(src: &str, n: &Names, release: Option<&Release>) -> Result<String, String> {
    let mut vars = vec![
        ("slug", n.slug.as_str()),
        ("kebab", n.kebab.as_str()),
        ("bundle_name", n.bundle_name.as_str()),
        ("bundle_id", n.bundle_id.as_str()),
        ("display_name", n.display_name.as_str()),
        ("version", app_meta::VERSION),
        ("libexec", n.linux_libexec_dir.as_str()),
        ("license", LICENSE),
        ("maintainer", MAINTAINER),
        ("maintainer_name", maintainer_name()),
        ("developer_id", developer_id(n)),
        ("homepage", app_meta::REPOSITORY),
    ];
    if let Some(r) = release {
        vars.push(("release_date", r.date.as_str()));
        vars.push(("release_type", r.kind));
    }
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
    // Only a template that asks for the release pays for the git call.
    let release = if text.contains("{{release_") {
        Some(current_release(root)?)
    } else {
        None
    };
    render_with(&text, n, release.as_ref())
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
        let r = dev("2026-11-05");
        let metainfo = render_with(&template("metainfo.xml.in"), &names(), Some(&r)).unwrap();
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
        assert!(plist.contains("<key>CFBundleName</key>\n\t<string>AbcDef</string>"));
        assert!(plist.contains("<key>CFBundleExecutable</key>\n\t<string>abc_def-window</string>"));
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
            let r = dev("2026-11-05");
            let text = render_with(&template(file), &names(), Some(&r)).unwrap();
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

    fn dev(date: &str) -> Release {
        Release {
            date: date.into(),
            kind: "development",
        }
    }

    #[test]
    fn changelog_with_only_unreleased_has_no_date() {
        let c = "# Changelog\n\n## [Unreleased]\n\n### Added\n- x\n";
        assert_eq!(changelog_release_date(c, "0.1.0"), Ok(None));
    }

    #[test]
    fn changelog_dated_heading_for_the_version() {
        let c = "## [Unreleased]\n\n## [0.2.0] - 2026-03-04\n\n## [0.1.0] - 2026-01-02\n";
        assert_eq!(
            changelog_release_date(c, "0.2.0"),
            Ok(Some("2026-03-04".into()))
        );
        assert_eq!(
            changelog_release_date(c, "0.1.0"),
            Ok(Some("2026-01-02".into()))
        );
    }

    #[test]
    fn changelog_dated_heading_for_another_version_only() {
        let c = "## [Unreleased]\n\n## [0.1.0] - 2026-01-02\n";
        assert_eq!(changelog_release_date(c, "0.2.0"), Ok(None));
        assert_eq!(changelog_release_date(c, "0.1"), Ok(None));
    }

    #[test]
    fn changelog_malformed_heading_is_an_error() {
        for bad in [
            "## [0.1.0] - 2026-1-2",
            "## [0.1.0] 2026-01-02",
            "## [0.1] - 2026-01-02",
            "## [0.1.0] - 2026-13-02",
            "## [Unrelased]",
        ] {
            let c = format!("## [Unreleased]\n\n{bad}\n");
            let e = changelog_release_date(&c, "0.1.0").unwrap_err();
            assert!(e.contains("malformed release heading"), "{bad}: {e}");
        }
    }

    #[test]
    fn pick_release_is_stable_with_a_dated_heading() {
        let c = format!(
            "## [Unreleased]\n\n## [{}] - 2026-03-04\n",
            app_meta::VERSION
        );
        let r = pick_release(&c, app_meta::VERSION, || panic!("git not needed")).unwrap();
        assert_eq!(
            r,
            Release {
                date: "2026-03-04".into(),
                kind: "stable"
            }
        );
    }

    #[test]
    fn pick_release_is_development_with_the_commit_date() {
        let r = pick_release("## [Unreleased]\n", "0.1.0", || Ok("2026-11-05".into())).unwrap();
        assert_eq!(r, dev("2026-11-05"));
    }

    #[test]
    fn pick_release_fails_when_git_cannot_date_it() {
        let e = pick_release("## [Unreleased]\n", "0.1.0", || Err("no git".into())).unwrap_err();
        assert_eq!(e, "no git");
        let e = pick_release("## [Unreleased]\n", "0.1.0", || Ok("yesterday".into())).unwrap_err();
        assert!(e.contains("not a YYYY-MM-DD date"), "{e}");
    }

    #[test]
    fn release_keys_need_a_release() {
        assert!(render("{{release_date}}", &names()).is_err());
        let r = dev("2026-11-05");
        assert_eq!(
            render_with("{{release_date}} {{release_type}}", &names(), Some(&r)).unwrap(),
            "2026-11-05 development"
        );
    }

    #[test]
    fn metainfo_has_release_date_homepage_and_developer() {
        let r = dev("2026-11-05");
        let m = render_with(&template("metainfo.xml.in"), &names(), Some(&r)).unwrap();
        let release = format!(
            "<release version=\"{}\" date=\"2026-11-05\" type=\"development\"/>",
            app_meta::VERSION
        );
        assert!(m.contains(&release), "{m}");
        assert!(m.contains(&format!(
            "<url type=\"homepage\">{}</url>",
            app_meta::REPOSITORY
        )));
        assert!(m.contains("<developer id=\"com.bawdo\">"));
        assert!(m.contains("<name>Keith Bawden</name>"));
        assert!(!m.contains('@'));
    }

    #[test]
    fn maintainer_name_drops_the_email() {
        assert_eq!(maintainer_name(), "Keith Bawden");
    }

    #[test]
    fn render_file_works_in_a_git_checkout() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let m = render_file(&root, "metainfo.xml.in", &current_names()).unwrap();
        assert!(m.contains("type=\""));
        assert!(!m.contains("{{"));
    }
}
