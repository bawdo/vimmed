# Release binary size: tree-sitter grammars

This records what the code-block grammars cost in the release `window` binary.
Task 14 added the highlighter with the Rust grammar only. Task 28 added the
other eleven grammars (Go, JavaScript, TypeScript and TSX, Ruby, SQL, HTML,
CSS, Bash, HCL, Markdown block and inline).

## Method

- Profile: `release` (the workspace default), macOS arm64.
- Tool: `cargo bloat --release -p window --crates`, then `ls -l` on
  `target/release/window`.
- At the base commit the `window` crate is an empty `fn main() {}` and does not
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

The real binary is about the size of the "Rust grammar only" harness build, not
the 13.0 MiB "all twelve grammars" build. No shell or window code creates a
`Highlighter` yet, so the linker drops the tree-sitter runtime and every grammar.
The 13 MiB figure should be expected once highlighting is wired into the view
path. Re-measure at that point.
