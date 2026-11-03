// Name derivation. Written with no `crate::` paths so `build.rs` can `include!` it
// as well as the library compiling it as a normal module.

/// Every spelling of the application name, derived from one slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    pub slug: String,
    pub kebab: String,
    pub display_name: String,
    pub bundle_name: String,
    pub bundle_id: String,
    pub env_prefix: String,
    pub window_bin: String,
    pub linux_libexec_dir: String,
}

impl Names {
    /// Derive all forms. The slug must match `^[a-z][a-z0-9_]*$`.
    pub fn from_slug(slug: &str, id_prefix: &str) -> Result<Names, String> {
        let mut chars = slug.chars();
        let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
        let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !first_ok || !rest_ok {
            return Err(format!(
                "invalid slug {slug:?}: must match ^[a-z][a-z0-9_]*$"
            ));
        }
        let kebab = slug.replace('_', "-");
        let bundle_name: String = slug
            .split('_')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut c = part.chars();
                match c.next() {
                    Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect();
        Ok(Names {
            slug: slug.to_string(),
            display_name: slug.to_string(),
            bundle_id: format!("{id_prefix}.{kebab}"),
            env_prefix: format!("{}_", slug.to_ascii_uppercase()),
            window_bin: format!("{slug}-window"),
            linux_libexec_dir: format!("/usr/lib/{slug}"),
            kebab,
            bundle_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_all_forms() {
        let n = Names::from_slug("abc_def", "com.bawdo").unwrap();
        assert_eq!(
            (
                n.kebab.as_str(),
                n.bundle_name.as_str(),
                n.bundle_id.as_str(),
                n.env_prefix.as_str(),
                n.window_bin.as_str()
            ),
            (
                "abc-def",
                "AbcDef",
                "com.bawdo.abc-def",
                "ABC_DEF_",
                "abc_def-window"
            )
        );
    }

    #[test]
    fn rejects_bad_slugs() {
        for s in ["", "Foo", "1x", "a-b", "a b"] {
            assert!(Names::from_slug(s, "x").is_err());
        }
    }
}
