//! `cargo xtask fixture [DIR]`: writes the 1,000-line test document and two PNGs.

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

![A gradient](img1.png)

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
}
