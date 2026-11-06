//! Key sequences in `<Esc>` notation, as the tests write them and the
//! `editor_keys` fuzz harness reads them.

use crate::input::{Key, Mods};

/// The key a `<name>` escape stands for: `Esc`, `CR`, `BS`, `Tab`, `Del`,
/// `Space`, `lt`, the arrows, `Home`, `End`, or a single char, after any
/// run of modifier prefixes: `S-` Shift, `C-` Ctrl, `M-` Alt (Option) and
/// `D-` Cmd (the primary modifier on the Mac, which the test editors run
/// as). So `S-Tab`, `C-Space`, `D-s`, `M-Left` and `D-S-z`. A bare char
/// needs a prefix (`<x>` is no escape).
fn named(name: &str) -> Option<(Key, Mods)> {
    let mut mods = Mods::default();
    let mut rest = name;
    loop {
        let flag = match rest.get(..2) {
            Some("S-") => &mut mods.shift,
            Some("C-") => &mut mods.ctrl,
            Some("M-") => &mut mods.alt,
            Some("D-") => &mut mods.cmd,
            _ => break,
        };
        *flag = true;
        rest = &rest[2..];
    }
    let key = match rest {
        "Esc" => Key::Esc,
        "CR" => Key::Enter,
        "BS" => Key::Backspace,
        "Tab" => Key::Tab,
        "Del" => Key::Delete,
        "Space" => Key::Space,
        "lt" => Key::Char('<'),
        "Left" => Key::Left,
        "Right" => Key::Right,
        "Up" => Key::Up,
        "Down" => Key::Down,
        "Home" => Key::Home,
        "End" => Key::End,
        _ => {
            let mut chars = rest.chars();
            let c = chars.next()?;
            if chars.next().is_some() || mods == Mods::default() {
                return None;
            }
            Key::Char(c)
        }
    };
    Some((key, mods))
}

/// The keys in `seq`, in order. `<Esc>`-style escapes name special keys
/// (see `named`); a `<` that starts no known escape is a literal `<`.
pub(crate) fn parse_keys(seq: &str) -> Vec<(Key, Mods)> {
    let mut out = Vec::new();
    let mut rest = seq;
    while let Some(c) = rest.chars().next() {
        let escape = c
            .eq(&'<')
            .then(|| rest.find('>'))
            .flatten()
            .and_then(|end| named(&rest[1..end]).map(|k| (k, end + 1)));
        let ((k, mods), used) = match escape {
            Some(found) => found,
            None if c == ' ' => ((Key::Space, Mods::default()), 1),
            None => ((Key::Char(c), Mods::default()), c.len_utf8()),
        };
        out.push((k, mods));
        rest = &rest[used..];
    }
    out
}
