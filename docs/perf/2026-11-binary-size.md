# Release binary size: tree-sitter grammars

This records what the code-block grammars cost in the release `window` binary.
Task 14 added the highlighter with the Rust grammar only. Task 28 added the
other eleven grammars (Go, JavaScript, TypeScript and TSX, Ruby, SQL, HTML,
CSS, Bash, HCL, Markdown block and inline).

## Method

- Profile: `release` (the workspace default), macOS arm64.
- Tool: `cargo bloat --release -p window --crates`, then `ls -l` on
  `target/release/window`.
- Historical (Task 28 time; see the Checkpoint 1 re-measure below for the real
  binary): at that commit the `window` crate is an empty `fn main() {}` and does not
  depend on `editor-render`, so a plain build measures nothing useful. For both
  measurements a throwaway edit (not committed) made `window` depend on
  `editor-render` and create a `Highlighter`, request one Rust block and poll
  it. That is enough to link the highlighter and every grammar it references.
  Both runs used the identical harness, so the difference is the grammars.
- cargo-bloat attributes only the `.text` section to crates. Grammar parse
  tables live in read-only data, so they show up in the file size and in the
  `[Unknown]` row rather than under each grammar crate. The file size is the
  figure to trust.

## Before (Rust grammar only)

`ls -l target/release/window`: 3,060,680 bytes (2.9 MiB).

| Crate            | .text size |
|------------------|-----------:|
| std              |  334.4 KiB |
| regex_automata   |  255.3 KiB |
| regex_syntax     |  126.5 KiB |
| [Unknown]        |   98.1 KiB |
| aho_corasick     |   81.4 KiB |
| tree_sitter      |   74.6 KiB |
| editor_render    |   18.6 KiB |
| window           |   14.4 KiB |
| memchr           |    8.4 KiB |

Total `.text`: 1.0 MiB. File size: 2.9 MiB.

## After (all twelve grammars)

`ls -l target/release/window`: 13,612,728 bytes (13.0 MiB).

| Crate            | .text size |
|------------------|-----------:|
| [Unknown]        |  865.2 KiB |
| std              |  339.7 KiB |
| regex_automata   |  255.2 KiB |
| regex_syntax     |  126.5 KiB |
| aho_corasick     |   81.4 KiB |
| tree_sitter      |   75.7 KiB |
| editor_render    |   21.5 KiB |
| window           |   15.2 KiB |
| tree_sitter_bash |   10.6 KiB |
| memchr           |    8.4 KiB |
| tree_sitter_ruby |    4.9 KiB |

Total `.text`: 1.8 MiB. File size: 13.0 MiB.

## Reading the numbers

- The grammars add about 10.5 MiB to the binary (3.06 MB to 13.61 MB), roughly
  4.4 times the size. Almost all of it is generated parse tables and lexer
  data, which is why `.text` grows by only 0.8 MiB while the file grows by
  10 MiB. The `[Unknown]` row growth (98 KiB to 865 KiB) is the part of that
  data cargo-bloat can see.
- The regex crates are pulled in by tree-sitter's query predicates and are the
  same before and after.
- Grammar data is mapped read-only and paged in on demand, so it costs disk
  and download size but little resident memory until a language is used. The
  parsers and queries are also only built when a block in that language is
  first requested.
- If the size becomes a problem, the options are a feature flag per grammar
  (ship fewer languages) or stripping and link-time optimisation on the
  release profile. Neither is done here.

## Re-measure on the real binaries (Checkpoint 1)

Date 2026-11-04, sha 0337930, macOS arm64, `cargo build --release`.

The numbers above came from a throwaway harness that forced `window` to link the
highlighter. This section measures the real `window` and `launcher` as they are
at Checkpoint 1.

| Binary | Bytes | Size |
|---|---:|---|
| `target/release/window` | 3,023,616 | 2.9 MiB |
| `target/release/launcher` | 337,184 | 329 KiB |

`cargo bloat --release -p window --crates -n 20`: `.text` 2.3 MiB, file 3.4 MiB.

| Crate | .text size |
|---|---:|
| std | 414.0 KiB |
| harfrust | 358.4 KiB |
| skrifa | 261.5 KiB |
| read_fonts | 183.6 KiB |
| zeno | 147.1 KiB |
| tiny_skia | 134.5 KiB |
| cosmic_text | 124.5 KiB |
| image_webp | 119.5 KiB |
| zune_jpeg | 112.0 KiB |
| image | 106.4 KiB |
| editor_core | 88.8 KiB |
| editor_render | 64.6 KiB |
| shell_mac | 47.2 KiB |
| swash | 42.7 KiB |
| png | 31.1 KiB |

All figures here are macOS arm64 only; Linux sizes are not measured.

The real binary (3.0 MB) is far below the 13.0 MiB "all twelve grammars"
harness build. No shell or window code creates a `Highlighter` yet, so the
linker drops the tree-sitter runtime and every grammar, the Rust grammar
included. Its size is coincidentally close to the "Rust grammar only" harness
figure, which did link that grammar, so do not read the two as the same content.
Expect the 13 MiB figure once highlighting is wired into the view path.
Re-measure at that point.

Evidence that the grammars are absent. The shipped release profile sets
`strip = true`, so `nm` on it shows only 178 symbols and proves nothing. The
check below used an unstripped build of the same sha (identical code,
`CARGO_PROFILE_RELEASE_STRIP=false cargo build --release -p window
--target-dir <dir>`, 3,587,128 bytes, 3,080 symbols):

```
$ nm <dir>/release/window | grep -ci tree_sitter
2
$ nm <dir>/release/window | grep -i "tree_sitter\|highlight\|Highlighter"
__RINvNtCs..._4core3ptr9drop_glueNtNtCs..._5alloc6string6StringECs..._11tree_sitter
__RNvXs1i_NtCs..._4core3fmtRjNtB6_7Display3fmtCs..._11tree_sitter
```

The two hits are core/alloc generics (`drop_glue<String>` and `Display for &T`)
instantiated inside the tree_sitter crate and merged by the linker; they are not
tree-sitter code. There is no `tree_sitter_rust` (or any other grammar) symbol,
no tree-sitter parser symbol, and no `Highlighter` symbol. On the shipped
(stripped) binary `nm target/release/window | grep -c tree_sitter` prints 0.

## Re-measure with grammars and font fallback linked (Checkpoint 1)

The 3,023,616 B figure above was measured at 0337930, before any shell used the
highlighter, so it describes a binary without the grammars. Since then Task 27b
wired the highlighter into the shells and Task 42 added the lazy OS font
fallback. The release `window` binary from `cargo xtask bundle-mac --release`
(macOS arm64):

| Sha | Change | `window` bytes |
|---|---|---:|
| 0337930 | grammars not linked | 3,023,616 |
| b7a1355 | grammars linked (Task 27b) | 15,943,184 |
| 8a937bf | plus lazy font fallback (Task 42) | 16,042,896 |

The launcher is unchanged at 337,184 B. The font fallback adds 99,712 B. This
is the jump flagged above: the harness estimate for the grammars alone was
13.6 MB, and the real window with the grammars and everything else it now links
is 15.9 MB (15.2 MiB).

Relation to cold start (macOS, Mac16,6, laptop display, see
`checkpoint-1.md`): against the Task 15 baseline at 935ed41, when the window
was about 3.0 MB, first_frame median went from 186.8 to 205.7 ms (+18.9 ms) and
p95 from 203.4 to 214.7 ms (+11.3 ms). Both are inside the 250 ms median and
400 ms p95 thresholds. Most of the growth is before first paint, about 10.7
of the 18.9 ms (toolkit_init 107.4 to 116.6 ms, window_created 129.4 to
140.1 ms, first_paint 133.1 to 143.8 ms). The baselines differ: those three
marks are compared with ff1a140, first_frame with 935ed41. That the extra time
comes from mapping and relocating a binary about five times larger is an
inference, not something the marks show. highlight_ready (179.5 ms median)
lands about 36 ms after first_paint, inside the window, so highlighting is part
of the later stretch to first_frame. If the inference holds, the size has a
measurable but affordable cost on cold start; the options listed above (a feature flag per grammar, link-time
optimisation) remain available if it needs to come down.
