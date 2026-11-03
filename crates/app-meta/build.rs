//! Reads the single name definition from the root manifest and exports every derived
//! form to the library as `APP_META_*` environment variables.

include!("src/derive.rs");

use std::path::Path;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed=src/derive.rs");

    let text = std::fs::read_to_string(&manifest).expect("read root Cargo.toml");
    let doc: toml::Table = text.parse().expect("parse root Cargo.toml");
    let app = doc
        .get("workspace")
        .and_then(|v| v.get("metadata"))
        .and_then(|v| v.get("app"))
        .expect("missing [workspace.metadata.app]");
    let field = |key: &str| -> &str {
        app.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("missing string `{key}` in [workspace.metadata.app]"))
    };

    let n = Names::from_slug(field("name"), field("id_prefix")).unwrap_or_else(|e| panic!("{e}"));
    for (key, value) in [
        ("SLUG", &n.slug),
        ("KEBAB", &n.kebab),
        ("DISPLAY_NAME", &n.display_name),
        ("BUNDLE_NAME", &n.bundle_name),
        ("BUNDLE_ID", &n.bundle_id),
        ("ENV_PREFIX", &n.env_prefix),
        ("WINDOW_BIN", &n.window_bin),
        ("LINUX_LIBEXEC_DIR", &n.linux_libexec_dir),
    ] {
        println!("cargo:rustc-env=APP_META_{key}={value}");
    }
}
