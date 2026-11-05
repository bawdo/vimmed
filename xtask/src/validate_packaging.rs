//! `validate-packaging`: renders the desktop entry and AppStream metainfo
//! and runs the Linux validators over them. The validators ship in the
//! check container, so `linux-check` runs this after the build.

use crate::templates;
use app_meta::derive::Names;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The rendered file names, desktop entry first, then the metainfo.
pub fn rendered_names(n: &Names) -> (String, String) {
    (
        format!("{}.desktop", n.bundle_id),
        format!("{}.metainfo.xml", n.bundle_id),
    )
}

fn not_found(tool: &str) -> String {
    format!("validate-packaging: {tool} not found; run it in the check container")
}

/// Runs `tool args... file`, echoing its output. A missing tool or a
/// non-zero exit is an error naming the tool and the file.
fn validate(tool: &str, args: &[&str], file: &Path) -> Result<(), String> {
    let out = Command::new(tool)
        .args(args)
        .arg(file)
        .output()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => not_found(tool),
            _ => format!("validate-packaging: {tool}: {e}"),
        })?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "validate-packaging: {tool} rejected {}",
            file.display()
        ))
    }
}

fn render_and_validate(root: &Path, dir: &Path) -> Result<(), String> {
    let n = templates::current_names();
    let (desktop, metainfo) = rendered_names(&n);
    let write = |name: &str, template: &str| -> Result<PathBuf, String> {
        let path = dir.join(name);
        std::fs::write(&path, templates::render_file(root, template, &n)?)
            .map_err(|e| format!("validate-packaging: write {}: {e}", path.display()))?;
        Ok(path)
    };
    let desktop = write(&desktop, "app.desktop.in")?;
    let metainfo = write(&metainfo, "metainfo.xml.in")?;
    validate("desktop-file-validate", &[], &desktop)?;
    validate(
        "appstreamcli",
        &["validate", "--no-net", "--explain"],
        &metainfo,
    )
}

/// Renders both files to a fresh temp directory and validates them.
pub fn run(root: &Path) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("validate-packaging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("validate-packaging: create {}: {e}", dir.display()))?;
    let result = render_and_validate(root, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_names_use_bundle_id() {
        let n = Names::from_slug("abc_def", "com.example").unwrap();
        assert_eq!(
            rendered_names(&n),
            (
                format!("{}.desktop", n.bundle_id),
                format!("{}.metainfo.xml", n.bundle_id)
            )
        );
    }
}
