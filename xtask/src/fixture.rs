//! `cargo xtask fixture [DIR]`: writes the 1,000-line test document and two PNGs.
//!
//! The first image sits right after the opening paragraphs (owner decision
//! D13). The renderer only decodes images on screen, and `images_painted`
//! is marked when one is shown, so the image must be wholly on the first
//! screen of a bench window, even one clamped to a short display (tested
//! at 700 pt). It used to be line 38, about 948 pt down the page, and was
//! never decoded in a window shorter than about 976 pt.

use std::fmt::Write as _;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const LINES: usize = 1000;

pub fn run(dir: Option<String>) -> ExitCode {
    let dir = PathBuf::from(dir.unwrap_or_else(|| "target/fixture".into()));
    match write_all(&dir) {
        Ok(()) => {
            println!("wrote {LINES} lines to {}", dir.join("test.md").display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask fixture: {e}");
            ExitCode::FAILURE
        }
    }
}

fn write_all(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    write_png(&dir.join("img1.png"), 1200, 700, false)?;
    write_png(&dir.join("img2.png"), 900, 900, true)?;
    std::fs::write(dir.join("test.md"), markdown())?;
    Ok(())
}

fn write_png(path: &Path, w: u32, h: u32, alt: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let a = (x * 255 / w) as u8;
            let b = (y * 255 / h) as u8;
            data.extend_from_slice(&if alt { [200, a, b] } else { [a, b, 200] });
        }
    }
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&data)?;
    Ok(())
}

const HEAD: &str = r#"# Startup fixture

This paragraph has **bold** text, some *italic* text and `inline code` in it.

A second paragraph runs long enough to wrap across the column, so line layout has to break it into more than one visual row when the window is at its default width.

![A gradient](img1.png)

- First item with **bold**
- Second item
  - Nested item with `code`
  - Another nested item
- [ ] A task
- [x] A done task

```rust
use std::collections::HashMap;

/// Count words in a string.
fn count_words(text: &str) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for word in text.split_whitespace() {
        *counts.entry(word).or_insert(0) += 1;
    }
    counts
}

fn main() {
    let c = count_words("a b a c b a");
    println!("{:?} {}", c, 42);
}
```

| Name | Kind | Size |
|------|------|------|
| alpha | file | 12 |
| beta | dir | 4 |
| gamma | link | 1 |

> A quoted line with *emphasis*.

---
"#;

/// The document: exactly `LINES` lines, newline terminated.
pub fn markdown() -> String {
    let mut s = String::from(HEAD);
    let mut section = 1;
    while s.lines().count() < LINES {
        writeln!(s, "## Section {section}\n").unwrap();
        writeln!(s, "Paragraph {section} with **strong words**, *a little emphasis* and a `snippet`, followed by enough plain prose to wrap at the column width.\n").unwrap();
        writeln!(s, "1. Ordered item one").unwrap();
        writeln!(s, "2. Ordered item two with *italic*").unwrap();
        writeln!(s, "3. Ordered item three\n").unwrap();
        if section == 20 {
            writeln!(s, "```bash\nfor f in *.md; do\n  echo \"$f\"\ndone\n```\n").unwrap();
        }
        if section == 40 {
            writeln!(s, "![A second gradient](img2.png)\n").unwrap();
        }
        writeln!(s, "> Quote in section {section}.\n").unwrap();
        section += 1;
    }
    let lines: Vec<&str> = s.lines().take(LINES).collect();
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_has_1000_lines_two_fences_one_table_two_images() {
        let md = markdown();
        assert_eq!(md.lines().count(), 1000);
        let fences = md.lines().filter(|l| l.starts_with("```")).count();
        assert_eq!(fences, 4, "two fenced blocks, open and close each");
        let tables = md.lines().filter(|l| l.starts_with("|---")).count();
        assert_eq!(tables, 1);
        assert_eq!(md.matches("](img").count(), 2);
        assert!(md.contains("](img1.png)"));
        assert!(md.contains("](img2.png)"));
    }

    struct NoHost;

    impl editor_core::host::Host for NoHost {
        fn clipboard_get(&mut self) -> Option<String> {
            None
        }
        fn clipboard_set(&mut self, _: &str) {}
        fn open_external(&mut self, _: &str) {}
        fn spawn_window(&mut self, _: editor_core::host::WindowRequest) {}
        fn now_ns(&self) -> u64 {
            0
        }
        fn waker(&self) -> editor_core::host::Waker {
            std::sync::Arc::new(|| {})
        }
    }

    /// D13: the first image must be on the first screen of a short bench
    /// window, so `paint_all` reserves it (queues its decode) and
    /// `images_painted` is logged.
    #[test]
    fn first_image_is_on_the_first_screen_of_a_700pt_window() {
        use editor_core::editor::{Config, Editor, OpenRequest};
        use editor_core::platform::Platform;
        use editor_core::view_model::ViewSource;
        use editor_render::fonts::{FontPaths, FontSet};
        use editor_render::layout_cosmic::CosmicLayout;
        use editor_render::view::DocView;
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("fixture-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_all(&dir).unwrap();
        let line = markdown()
            .lines()
            .position(|l| l.contains("](img1.png)"))
            .expect("first image");

        let editor = Editor::open(
            OpenRequest {
                path: dir.join("test.md"),
                view: false,
                display_name: None,
            },
            Config::default(),
            Platform::current(),
            Box::new(NoHost),
        );
        let fonts_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/editor-render/tests/fonts");
        let fonts = FontSet::load(&FontPaths {
            sans: fonts_dir.join("NotoSans-Regular.ttf"),
            mono: fonts_dir.join("NotoSansMono-Regular.ttf"),
            fallback: Vec::new(),
        });
        let tokens = editor.tokens().clone();
        // The bench window's width, 700 pt tall, at Retina scale.
        let layout = Box::new(CosmicLayout::new(fonts, &tokens));
        let mut view = DocView::new(layout, &tokens, 900.0, 700.0, 2.0);
        view.set_images(&dir, std::sync::Arc::new(|| {}));
        view.paint_all(&editor);
        assert!(
            view.is_shaped(line),
            "image line {} is below the first screen of a 700 pt window",
            line + 1
        );
        // And all of it fits above the status bar.
        let px = |id| (tokens.px(id) * 2.0).round();
        let top = px(editor_core::tokens::TokenId::SpacePageTop);
        let bottom: f32 = top
            + (0..=line)
                .map(|i| view.line_height_px(i).unwrap())
                .sum::<f32>();
        let text_bottom = 700.0 * 2.0 - px(editor_core::tokens::TokenId::SizeStatusHeight);
        assert!(
            bottom <= text_bottom,
            "image ends at {} pt, past the text area's {} pt",
            bottom / 2.0,
            text_bottom / 2.0
        );
        // Laid out means reserved: the decode was queued and lands.
        let start = Instant::now();
        let mut painted = false;
        while !painted && start.elapsed() < Duration::from_secs(10) {
            painted = view.poll_async();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(painted, "the first image's decode never landed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
