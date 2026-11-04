//! The system's light or dark, for `ThemePref::System`.
//!
//! GTK 4.20 and later reads the desktop portal itself and says what it
//! found in `gtk-interface-color-scheme`, which is read at start and
//! watched. Before 4.20 the portal's `org.freedesktop.appearance`
//! `color-scheme` is read over D-Bus, asynchronously, once the first frame
//! is out, and its `SettingChanged` signal is watched. Each scheme found
//! is reported to the shell, which caches it in the state directory
//! (`editor_core::config::cache_appearance`) so the next start paints its
//! first frame in it without waiting.
//!
//! No session bus or no portal (a container) is quiet: nothing is
//! reported, and the cached (or light) theme stays.
//!
//! The scheme values are pure and build and test on every platform; the
//! watching is compiled on Linux only.

use editor_core::tokens::Theme;

/// The desktop portal's bus name, object path and settings interface.
pub const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
pub const SETTINGS_IFACE: &str = "org.freedesktop.portal.Settings";
/// The setting's namespace and key.
pub const APPEARANCE_NS: &str = "org.freedesktop.appearance";
pub const COLOR_SCHEME: &str = "color-scheme";
/// The `GtkSettings` property GTK 4.20 added.
pub const INTERFACE_SCHEME: &str = "gtk-interface-color-scheme";

/// The portal's `color-scheme`: 1 prefers dark; 2 prefers light and 0 has
/// no preference, both light.
pub fn theme_of_scheme(scheme: u32) -> Theme {
    if scheme == 1 {
        Theme::Dark
    } else {
        Theme::Light
    }
}

/// `gtk-interface-color-scheme` by its nick: `dark`, `light`, `default`
/// (no preference, light) or `unsupported` (GTK could not tell: `None`).
pub fn theme_of_interface_scheme(nick: &str) -> Option<Theme> {
    match nick {
        "dark" => Some(Theme::Dark),
        "light" | "default" => Some(Theme::Light),
        _ => None,
    }
}

/// True when this GTK (the run-time version) has
/// `gtk-interface-color-scheme`: 4.20 and later.
pub fn has_interface_scheme(major: u32, minor: u32) -> bool {
    (major, minor) >= (4, 20)
}

#[cfg(target_os = "linux")]
pub use watch::{follow, initial};

#[cfg(target_os = "linux")]
mod watch {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gtk::prelude::*;
    use gtk::{gio, glib};

    use super::*;

    /// How long a portal call may take. It never holds a frame back.
    const CALL_TIMEOUT_MS: i32 = 2000;

    thread_local! {
        /// The `SettingChanged` subscription, kept for the process's life.
        static SUBSCRIPTION: RefCell<Option<gio::SignalSubscription>> =
            const { RefCell::new(None) };
    }

    type Report = Rc<dyn Fn(Theme)>;

    fn settings_scheme_known() -> bool {
        has_interface_scheme(gtk::major_version(), gtk::minor_version())
    }

    /// `gtk-interface-color-scheme` as a theme, where GTK has it and knows.
    fn read_settings(settings: &gtk::Settings) -> Option<Theme> {
        settings.find_property(INTERFACE_SCHEME)?;
        let value = settings.property_value(INTERFACE_SCHEME);
        let (_, scheme) = glib::EnumValue::from_value(&value)?;
        theme_of_interface_scheme(scheme.nick())
    }

    /// The system's theme now, where GTK can say without waiting (4.20
    /// and later). Call it after `gtk::init`.
    pub fn initial() -> Option<Theme> {
        if !settings_scheme_known() {
            return None;
        }
        read_settings(&gtk::Settings::default()?)
    }

    /// Starts following the system's light or dark: `report` hears each
    /// scheme found, never from inside this call.
    pub fn follow(report: impl Fn(Theme) + 'static) {
        let report: Report = Rc::new(report);
        if settings_scheme_known()
            && let Some(settings) = gtk::Settings::default()
            && settings.find_property(INTERFACE_SCHEME).is_some()
        {
            let _ = settings.connect_notify_local(Some(INTERFACE_SCHEME), move |s, _| {
                if let Some(theme) = read_settings(s) {
                    report(theme);
                }
            });
            return;
        }
        gio::bus_get(gio::BusType::Session, gio::Cancellable::NONE, move |bus| {
            // No session bus: keep what is painted.
            let Ok(bus) = bus else {
                return;
            };
            let changed = report.clone();
            let sub = bus.subscribe_to_signal(
                Some(PORTAL_BUS),
                Some(SETTINGS_IFACE),
                Some("SettingChanged"),
                Some(PORTAL_PATH),
                Some(APPEARANCE_NS),
                gio::DBusSignalFlags::NONE,
                move |signal| {
                    let args = signal.parameters.get::<(String, String, glib::Variant)>();
                    if let Some((ns, key, value)) = args
                        && ns == APPEARANCE_NS
                        && key == COLOR_SCHEME
                        && let Some(scheme) = scheme_of(&value)
                    {
                        changed(theme_of_scheme(scheme));
                    }
                },
            );
            SUBSCRIPTION.with(|s| *s.borrow_mut() = Some(sub));
            read(&bus, "ReadOne", report);
        });
    }

    /// Reads `color-scheme` with `method`: `ReadOne`, or `Read` from a
    /// portal too old to have it. A portal that is not there is quiet.
    fn read(bus: &gio::DBusConnection, method: &'static str, report: Report) {
        let args = (APPEARANCE_NS, COLOR_SCHEME).to_variant();
        let again = bus.clone();
        bus.call(
            Some(PORTAL_BUS),
            PORTAL_PATH,
            SETTINGS_IFACE,
            method,
            Some(&args),
            None,
            gio::DBusCallFlags::NONE,
            CALL_TIMEOUT_MS,
            gio::Cancellable::NONE,
            move |reply| match reply {
                Ok(reply) => {
                    if let Some(scheme) = reply.try_child_value(0).as_ref().and_then(scheme_of) {
                        report(theme_of_scheme(scheme));
                    }
                }
                Err(_) if method == "ReadOne" => read(&again, "Read", report),
                Err(_) => {}
            },
        );
    }

    /// The `u32` inside however many variant boxes (`Read` adds one).
    fn scheme_of(value: &glib::Variant) -> Option<u32> {
        value
            .get::<u32>()
            .or_else(|| scheme_of(&value.as_variant()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_scheme_one_is_dark_and_the_rest_light() {
        assert_eq!(theme_of_scheme(1), Theme::Dark);
        assert_eq!(theme_of_scheme(2), Theme::Light);
        assert_eq!(theme_of_scheme(0), Theme::Light);
        assert_eq!(theme_of_scheme(7), Theme::Light);
    }

    #[test]
    fn interface_scheme_nicks() {
        assert_eq!(theme_of_interface_scheme("dark"), Some(Theme::Dark));
        assert_eq!(theme_of_interface_scheme("light"), Some(Theme::Light));
        assert_eq!(theme_of_interface_scheme("default"), Some(Theme::Light));
        assert_eq!(theme_of_interface_scheme("unsupported"), None);
    }

    #[test]
    fn gtk_has_the_interface_scheme_from_4_20() {
        assert!(!has_interface_scheme(4, 14));
        assert!(!has_interface_scheme(4, 19));
        assert!(has_interface_scheme(4, 20));
        assert!(has_interface_scheme(4, 22));
        assert!(has_interface_scheme(5, 0));
    }
}
