//! The help `:help` shows: moon_edit's cheat sheet, section by section,
//! with this editor's additions (`e`, `C`, `D`, named registers, `.`, the
//! extra quit commands, reader view and the link prompt), without `:img`,
//! and a footer saying compound counts multiply. Every string here is
//! user-visible text.

/// One section of the help: its title and its rows of keys and what they
/// do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelpSection {
    pub title: &'static str,
    pub rows: &'static [(&'static str, &'static str)],
}

/// The overlay's heading.
pub const TITLE: &str = "Vim-Lite Reference";

/// The line under the sections.
pub const FOOTER: &str = "Compound counts multiply: 2d3w deletes 6 words";

const SECTIONS: &[HelpSection] = &[
    HelpSection {
        title: "Modes",
        rows: &[
            ("Esc", "Normal mode"),
            ("i / a", "Insert before / after"),
            ("o / O", "Open line below / above"),
            ("v / V", "Visual / Visual Line"),
            ("Cmd/Ctrl+Shift+R", "Reader view on / off"),
        ],
    },
    HelpSection {
        title: "Counts",
        rows: &[
            ("[N]motion", "Repeat motion N times (3j, 5w)"),
            ("[N]op", "Repeat operator N times (3dd, 2yy)"),
            ("[N]G/gg", "Go to line N"),
            ("[N]x", "Delete N characters"),
            ("[N]p", "Paste N times"),
        ],
    },
    HelpSection {
        title: "Movement",
        rows: &[
            ("h j k l", "Left, down, up, right"),
            ("w / b", "Word forward / back"),
            ("e", "Word end"),
            ("0 / ^", "Line start / first non-blank"),
            ("$", "Line end"),
            ("gg / G", "Document start / end"),
        ],
    },
    HelpSection {
        title: "Search",
        rows: &[
            ("/", "Search forward"),
            ("?", "Search backward"),
            ("n", "Next match"),
            ("N", "Previous match"),
        ],
    },
    HelpSection {
        title: "Commands",
        rows: &[
            (":w", "Save file"),
            (":q", "Quit (fails if unsaved)"),
            (":q!", "Force quit without saving"),
            (":wq / :x", "Save and quit"),
            ("ZZ", "Save if changed and quit"),
            ("ZQ", "Quit without saving"),
            (":qa", "Same as :q (also :qa! and :wqa)"),
            (":cq", "Quit without saving, reporting failure"),
            (":e!", "Reload from disk, dropping changes"),
            (":help", "Show this help"),
            (":table N M", "Insert NxM table"),
            (":table", "Open table grid picker"),
        ],
    },
    HelpSection {
        title: "Find & Replace",
        rows: &[
            (":s/a/b/", "Replace first \"a\" with \"b\" on line"),
            (":%s/a/b/g", "Replace all \"a\" with \"b\" in file"),
            ("g flag", "Replace all matches (not just first)"),
            ("i flag", "Case insensitive matching"),
            ("c flag", "Confirm each replacement"),
            ("n flag", "Count matches only (no replace)"),
        ],
    },
    HelpSection {
        title: "Editing",
        rows: &[
            ("x", "Delete character"),
            ("dd / dw", "Delete line / word"),
            ("cc / cw", "Change line / word"),
            ("yy / yw", "Yank line / word"),
            ("C", "Change to line end"),
            ("D", "Delete to line end"),
            ("yic/dic/cic", "Yank/delete/change inner code"),
            ("p / P", "Paste after / before"),
            (
                "\"a..\"z",
                "Named register for the next yank, delete or paste",
            ),
            (".", "Repeat last change"),
            ("u / Ctrl+r", "Undo / redo"),
            ("Cmd/Ctrl+L", "Insert a link"),
        ],
    },
    HelpSection {
        title: "Lists",
        rows: &[
            ("gx", "Toggle task checkbox"),
            ("Tab", "Indent list item"),
            ("Shift+Tab", "Outdent list item"),
            ("Ctrl+Space", "Toggle task checkbox"),
        ],
    },
    HelpSection {
        title: "Tables",
        rows: &[
            (":table N M", "Insert NxM table"),
            (":table", "Open table grid picker"),
            ("Tab / S-Tab", "Next / previous cell"),
            ("Enter", "Move down in column (Insert)"),
            ("ta / tA", "Add row below / above"),
            ("td", "Delete row"),
            ("tD", "Delete table"),
            ("tc / tC", "Add column right / left"),
            ("tx", "Delete column"),
            ("t> / t<", "Move column right / left"),
            ("t=", "Cycle column alignment"),
        ],
    },
];

/// The help's sections, in order.
pub fn sections() -> &'static [HelpSection] {
    SECTIONS
}

/// One row of the help as the overlay shows it, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpRow {
    Title(&'static str),
    Section(&'static str),
    /// Keys and what they do.
    Entry(&'static str, &'static str),
    /// A gap between sections.
    Blank,
    Footer(&'static str),
}

/// The help, row by row: the title, each section's title and entries with
/// a gap after it, then the footer. The overlay scrolls by these rows.
pub fn rows() -> Vec<HelpRow> {
    let mut out = vec![HelpRow::Title(TITLE), HelpRow::Blank];
    for s in SECTIONS {
        out.push(HelpRow::Section(s.title));
        out.extend(s.rows.iter().map(|&(k, d)| HelpRow::Entry(k, d)));
        out.push(HelpRow::Blank);
    }
    out.push(HelpRow::Footer(FOOTER));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_text() -> Vec<&'static str> {
        rows()
            .into_iter()
            .flat_map(|r| match r {
                HelpRow::Title(t) | HelpRow::Section(t) | HelpRow::Footer(t) => vec![t],
                HelpRow::Entry(k, d) => vec![k, d],
                HelpRow::Blank => vec![],
            })
            .collect()
    }

    #[test]
    fn help_footer_states_counts_multiply() {
        assert_eq!(FOOTER, "Compound counts multiply: 2d3w deletes 6 words");
        assert_eq!(rows().last(), Some(&HelpRow::Footer(FOOTER)));
        assert!(all_text().iter().all(|t| !t.contains("not supported")));
    }

    #[test]
    fn sections_follow_the_cheat_sheet() {
        let titles: Vec<&str> = sections().iter().map(|s| s.title).collect();
        assert_eq!(
            titles,
            [
                "Modes",
                "Counts",
                "Movement",
                "Search",
                "Commands",
                "Find & Replace",
                "Editing",
                "Lists",
                "Tables"
            ]
        );
        let keys: Vec<&str> = sections()
            .iter()
            .flat_map(|s| s.rows.iter().map(|(k, _)| *k))
            .collect();
        assert!(!keys.iter().any(|k| k.contains(":img")));
        for want in [
            "e",
            "C",
            "D",
            "\"a..\"z",
            ":e!",
            ":cq",
            ":qa",
            "ZZ",
            "ZQ",
            ".",
            "gx",
            "Ctrl+Space",
        ] {
            assert!(keys.contains(&want), "{want} missing from {keys:?}");
        }
        assert!(keys.iter().any(|k| k.contains("Shift+R")), "reader toggle");
        assert!(keys.iter().any(|k| k.ends_with("+L")), "link prompt");
    }

    #[test]
    fn rows_lay_out_title_sections_and_footer() {
        let rows = rows();
        assert_eq!(rows[0], HelpRow::Title(TITLE));
        let entries = rows
            .iter()
            .filter(|r| matches!(r, HelpRow::Entry(..)))
            .count();
        let want: usize = sections().iter().map(|s| s.rows.len()).sum();
        assert_eq!(entries, want);
        let heads = rows
            .iter()
            .filter(|r| matches!(r, HelpRow::Section(_)))
            .count();
        assert_eq!(heads, sections().len());
    }
}
