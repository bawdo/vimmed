//! The process environment: the renderer default set at start-up, and what
//! child processes must not inherit.

use std::ffi::OsStr;

/// Renderer variables a person (or the bench) may set to choose a renderer.
pub const GSK_RENDERER: &str = "GSK_RENDERER";
pub const GDK_DISABLE: &str = "GDK_DISABLE";
/// What `GDK_DISABLE` is set to by default: GTK's Vulkan renderer costs tens
/// of milliseconds at start-up and gains nothing for a few textures.
pub const DISABLE_VULKAN: &str = "vulkan";

/// Variables stripped from every child: the renderer choice and GDK debug
/// flags (meant for this process only), plus anything with our env prefix.
const CHILD_STRIP: [&str; 3] = [GDK_DISABLE, "GDK_DEBUG", GSK_RENDERER];

/// True when the shell should set `GDK_DISABLE=vulkan`: only when neither
/// renderer variable is already set.
pub fn default_vulkan_off(gsk_renderer_set: bool, gdk_disable_set: bool) -> bool {
    !gsk_renderer_set && !gdk_disable_set
}

/// True when a child must not inherit `name`.
pub fn strip_from_child(name: &OsStr, prefix: &str) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    CHILD_STRIP.contains(&name) || (!prefix.is_empty() && name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vulkan_is_turned_off_only_by_default() {
        assert!(default_vulkan_off(false, false));
        assert!(!default_vulkan_off(true, false));
        assert!(!default_vulkan_off(false, true));
        assert!(!default_vulkan_off(true, true));
    }

    #[test]
    fn children_lose_renderer_debug_and_prefixed_variables() {
        let strip = |n: &str| strip_from_child(OsStr::new(n), "ABC_DEF_");
        assert!(strip("GDK_DISABLE"));
        assert!(strip("GDK_DEBUG"));
        assert!(strip("GSK_RENDERER"));
        assert!(strip("ABC_DEF_MARKS"));
        assert!(strip("ABC_DEF_BENCH"));
        assert!(!strip("ABC_DEFX"));
        assert!(!strip("GDK_BACKEND"));
        assert!(!strip("WAYLAND_DISPLAY"));
        assert!(!strip("PATH"));
    }
}
