# Fast font loading with correct fallback, and fast syntax highlighting start-up (cosmic-text 0.19, fontdb, swash, tree-sitter 0.27)

Research date: 2026-11-03. Sources are primary (crate source on GitHub `main`, docs.rs, crates.io API) unless noted. Some findings come from a local Core Text probe run on this Mac during research (Swift, `-O`, macOS Darwin 27.2); those are labelled "local probe". Claims I could not verify this session are kept in the Gaps sections.

## Fonts: system font discovery and fallback without a full scan

### Takeaway
The cheapest correct pattern is: load only the configured faces at start-up, then resolve fallback lazily by asking the OS (Core Text `CTFontCreateForString` on macOS, fontconfig `FcFontMatch`/`FcFontSort` with a charset on Linux) for a face that covers the missing character, and load just that file into fontdb. Your tofu bug is explained by cosmic-text's fallback design: its last resort is brute-force iteration over every face already in the fontdb, so with a trimmed db there is nothing to fall back to. cosmic-text `main` (unreleased after 0.19.0) has gained a persistent on-disk font index keyed on directory and file mtimes, which matches the caching pattern you prototyped.

### Cited Findings

**cosmic-text (0.19.0, released 2026-04-22)**
- 0.19.0 is the current release (published 2026-04-22; previous 0.18.2 on 2026-02-20). It depends on fontdb `^0.23`, shapes with HarfRust, rasterises with swash, and its fallback is "a custom implementation that reuses some static fallback lists from browsers such as Chromium and Firefox"; source: [docs.rs cosmic-text](https://docs.rs/crate/cosmic-text/latest); [GitHub releases API](https://api.github.com/repos/pop-os/cosmic-text/releases)
- `FontSystem::new()` docs warn: "This function takes some time to run. On the release build, it can take up to a second, while debug builds can take up to ten times longer", and recommend creating it once and sharing it; source: [cosmic-text src/font/system.rs](https://github.com/pop-os/cosmic-text/blob/main/src/font/system.rs)
- `FontSystem::new` hardcodes default generic families `Noto Sans Mono`, `Open Sans`, `DejaVu Serif` ("TODO: configurable default fonts"), then calls `new_with_locale_and_db_and_fallback(locale, db, PlatformFallback)`. `new_with_locale_and_db` / `new_with_locale_and_db_and_fallback` are the public escape hatch to supply your own trimmed `fontdb::Database` and your own `Fallback` impl; source: [system.rs](https://github.com/pop-os/cosmic-text/blob/main/src/font/system.rs)
- At construction, cosmic-text iterates `db.faces()` to build a list of monospace face IDs (filtering `face_info.monospaced`), so construction cost scales with the number of faces in the db; source: [system.rs](https://github.com/pop-os/cosmic-text/blob/main/src/font/system.rs)
- The `Fallback` trait has three methods: `common_fallback()`, `forbidden_fallback()`, `script_fallback(script, locale)`; each returns family names (static `&str`). A custom impl is passed to `new_with_locale_and_db_and_fallback`; source: [cosmic-text src/font/fallback/mod.rs](https://github.com/pop-os/cosmic-text/blob/main/src/font/fallback/mod.rs)
- Fallback order in `FontFallbackIter`: default/requested family, then script-specific families, then `common_fallback` families, then finally *every remaining face in the font-match key list* not in the forbidden list ("TODO: do not evaluate fonts more than once!"). Every family lookup is by name against faces already in the fontdb; it never asks the OS; source: [fallback/mod.rs](https://github.com/pop-os/cosmic-text/blob/main/src/font/fallback/mod.rs)
- macOS `PlatformFallback::common_fallback` is `[".SF NS", "Menlo", "Apple Color Emoji", "Geneva", "Arial Unicode MS"]`, forbidden is `[".LastResort"]`, and script fallbacks are static (e.g. Arabic to "Geeza Pro", Hebrew to "Arial", Han by locale to PingFang SC/TC/HK, Hiragino Sans, Apple SD Gothic Neo); source: [cosmic-text src/font/fallback/macos.rs](https://github.com/pop-os/cosmic-text/blob/main/src/font/fallback/macos.rs)
- Note "Apple Symbols" is not in the macOS common list. Local probe: `CTFontCreateForString(SF Mono, "U+2610")` returns family **Apple Symbols** at `/System/Library/Fonts/Apple Symbols.ttf`; U+2713 and U+2192 resolve to Lucida Grande, U+1F600 to Apple Color Emoji, CJK to Hiragino Sans - local probe (script reproduced in Inferences)
- **Persistent font cache on `main` (not in 0.19.0):** commit "feat: persistent font cache" (2026-06) adds `FontSystem::new_cached()`, `new_with_fonts_and_cache()`, `new_with_fonts_and_cache_path()` and `default_cache_path()` (`<cache-dir>/cosmic-text/fonts.cache`). The module docs: "`load_system_fonts` walks every system font directory and `mmap`s + parses each font file ... This module persists the small, parsed `fontdb::FaceInfo` set to disk and reloads it via `Database::push_face_info` instead of re-parsing every font file. It is a pure-Rust, dependency-free reimplementation of what fontconfig's `fc-cache` does. The cache is keyed on a cheap fingerprint of the font directories and files that were scanned (per-directory and per-file modification time, plus file size). On any mismatch the normal scan runs and the cache is rewritten atomically." Binary format with magic `CTFC` and a version number; source: [cosmic-text src/font/cache.rs](https://github.com/pop-os/cosmic-text/blob/main/src/font/cache.rs); [commit history for cache.rs](https://api.github.com/repos/pop-os/cosmic-text/commits?path=src/font/cache.rs)
- fontdb exposes `push_face_info` (used by that cache), so an application can implement the same pattern against fontdb 0.23 today without waiting for the next cosmic-text release; source: [cache.rs](https://github.com/pop-os/cosmic-text/blob/main/src/font/cache.rs)

**parley / fontique (0.12.0, released 2026-10-09)**
- parley 0.12.0 released 2026-10-09 and depends on fontique `^0.12.0`; uses HarfRust for shaping; MSRV Rust 1.88; source: [docs.rs parley](https://docs.rs/crate/parley/latest)
- fontique's `system` feature (default) enables system font backends. On Linux the `fontconfig-dlopen` feature lets it build without fontconfig installed, but "font fallback to system fonts will not work in this case"; source: [fontique README](https://github.com/linebender/parley/tree/main/fontique)
- **fontique on Linux** (`backend/fontconfig.rs`): calls `FcInitLoadConfig` then `FcConfigBuildFonts` (so it builds its own config, not the shared `FcInitLoadConfigAndFonts` global), reads `FcConfigGetFonts(FcSetSystem)` to build a family-name map only (no font files opened), and populates generic families by running `FcConfigSubstitute` plus `FcFontSort(trim = true)` for each generic name. Per-family face lists are resolved lazily on first request (`family()` caches into `family_map`), using `FcFontSort` and stopping at the first non-matching family. Fallback builds a pattern with an `FC_LANG` langset and an `FC_CHARSET` built from per-script sample characters (cached per script in `ScriptCharSetMap`), substitutes, then `FcFontMatch`; source: [fontique src/backend/fontconfig.rs](https://github.com/linebender/parley/blob/main/fontique/src/backend/fontconfig.rs)
- **fontique on macOS** (`backend/coretext.rs`) is *not* lazy for enumeration: `SystemFonts::new()` enumerates `CTFontCollection::from_available_fonts`, collects every font file URL, adds a recursive scan of all `Library/Fonts` directories (because "Apple hides certain fonts from CTFontCollection (notably SFNS.ttf)"), then runs `scan::ScannedCollection::from_paths`, which mmaps and parses each file with read-fonts. Fallback is lazy and per-script: it calls `CTFontCreateForString` (or `...WithLanguage` when a locale is given) with the script's sample text against a base font, then maps the returned family name back into its own name map. Generic families are hardcoded (e.g. SansSerif to Helvetica, Monospace to Courier/Courier New, SystemUi to "System Font"/".SF NS"); source: [fontique src/backend/coretext.rs](https://github.com/linebender/parley/blob/main/fontique/src/backend/coretext.rs); [fontique src/scan.rs](https://github.com/linebender/parley/blob/main/fontique/src/scan.rs)
- The combined Core Text plus directory scan was introduced in Feb 2026 (PR #568, follow-up to #536). A March 2026 change (#598) falls back to Heiti SC/TC because fontique cannot render PingFangUI's `hvgl` outlines; source: [GitHub commits for coretext.rs](https://api.github.com/repos/linebender/parley/commits?path=fontique/src/backend/coretext.rs)
- fontique fallback is keyed by script plus locale, not by individual codepoint. A symbol like U+2610 is script "Common", so whether it is covered depends on what the sample-text query returns for that key; source: [coretext.rs](https://github.com/linebender/parley/blob/main/fontique/src/backend/coretext.rs) (behaviour inferred from code; not tested)

**Core Text costs (local probe on this Mac)**
- Creating the base CTFont (`CTFontCreateWithName("SF Mono")`) took about 4 to 6 ms in a fresh process; the *first* `CTFontCreateForString` call took about 13.7 ms (Core Text cascade warm-up); subsequent calls for other strings took 0.01 to 0.4 ms. `CTFontCopyDefaultCascadeListForLanguages(SF Mono, ["en"])` took 0.06 ms after warm-up and returned 39 descriptors, beginning Lucida Grande, ".Apple Symbols Fallback", Hiragino Sans, Hiragino Sans GB, PingFang SC/TC/HK/MO - local probe

**Zed / GPUI**
- Zed's Linux/wgpu text system (`crates/gpui_wgpu/src/cosmic_text_system.rs`) is built on cosmic-text and its default constructor calls `FontSystem::new()` (full system scan); a separate `new_without_system_fonts` constructor uses `FontSystem::new_with_locale_and_db`. It keeps a `user_fallback_chain` and a `fallback_chain` of `(FontId, family)`; source: [Zed cosmic_text_system.rs](https://github.com/zed-industries/zed/blob/main/crates/gpui_wgpu/src/cosmic_text_system.rs)
- GPUI exposes per-font `FontFallbacks` ("Font families to try, in order, when a Font lacks a glyph"), a `buffer_font_fallbacks` user setting merged with platform defaults, and defaults that grew to include `.SystemUIFont`, Arial, Adwaita Sans, and `.ZedSans`; source: [gpui font_fallbacks.rs (docs.rs)](https://docs.rs/crate/gpui/0.2.0/source/src/text_system/font_fallbacks.rs); [Zed #15306](https://git.secluded.site/zed/commit/a1bd7a129710dbc42709a3a878a551e347bc2134); [Zed #35086](https://git.secluded.site/zed/commit/83f9f9d9e3f5914392cab9a09e3472711a1d7b38); [Zed #40129](https://git.secluded.site/zed/commit/9a463308096aab5b19ce2dabca334d27f4d43f22)

### Inferences
- **Root cause of the tofu:** with only SF Pro and SF Mono in the db, cosmic-text's fallback chain (script list, then common list, then every face in the db) has nothing to try, because none of `.SF NS`, Menlo, Apple Color Emoji, Geneva, Arial Unicode MS or Apple Symbols is loaded. With the full scan it "worked" only because the final brute-force pass eventually hit a face covering U+2610, and that pass (calling `get_font` on faces one by one, each a lazy parse) is very likely the bulk of your 11 to 57 ms first-layout jump. The full scan is not giving you good fallback; it gives you exhaustive fallback.
- **Recommended pattern for vimmed on cosmic-text 0.19:**
  1. Start-up: build a `fontdb::Database` with only the configured faces (paths from your cache file), plus a small, platform-specific "always present" fallback set loaded by path: on macOS `Apple Color Emoji`, `Apple Symbols`, `Menlo` (and optionally `Lucida Grande`); on Linux the fontconfig matches for `emoji` and `symbol`. Construct with `new_with_locale_and_db_and_fallback` and a custom `Fallback` whose common list names exactly those faces, so the brute-force pass is never reached in practice.
  2. On a missed codepoint (detect via glyph id 0 after shaping, or by checking the face's cmap before layout), ask the OS for a covering face on a background thread: macOS `CTFontCreateForString` with your primary font as the base, read `kCTFontURLAttribute`, then `db.load_font_file(path)` and invalidate the affected lines (note `db_mut()` clears cosmic-text's `font_matches_cache`). Linux: `FcPatternAddCharSet` with the missing codepoint, `FcConfigSubstitute`, `FcFontMatch` (or `FcFontSort` for a list), read `FC_FILE`. Remember the codepoint-to-path mapping in your on-disk cache so the next launch preloads it.
  3. Prewarm Core Text on a background thread right after first paint (a throwaway `CTFontCreateForString`) to absorb its ~14 ms first-call cost off the critical path.
- **Caching:** your "matched paths in a file" approach is the established pattern. cosmic-text `main` now does the same thing for the whole fontdb index, keyed on per-directory and per-file mtime plus size. For a trimmed index, key the cache on: fontconfig config file mtimes (`/etc/fonts/fonts.conf`, `conf.d`, `~/.config/fontconfig`), the mtimes of the font directories, the mtime/size of each cached font file, and the user's configured font families. Validate cheaply (a handful of `stat` calls) and refresh in the background if stale rather than blocking start-up.
- **fontique vs cosmic-text for fallback quality:** fontique's design (ask the OS per script, lazy family resolution on Linux) is closer to what browsers do, but on macOS it still performs a full eager scan of every font file at `SystemFonts::new()`, so swapping to parley would not by itself remove the macOS start-up cost you measured.
- Probe source (for reproducibility): Swift script calling `CTFontCreateWithName`, `CTFontCreateForString`, `CTFontCopyAttribute(kCTFontURLAttribute)`, `CTFontCopyDefaultCascadeListForLanguages`, timed with `Date()`; compiled with `swiftc -O`. Single runs, so treat timings as indicative.

### Gaps
- **Alacritty (crossfont), WezTerm, Ghostty, kitty, Chromium/Skia:** not verified this session (tool budget). From background knowledge only, so treat as hypotheses to check in source: crossfont uses Core Text on macOS and fontconfig `FcFontSort` on Linux to build a fallback list; Ghostty uses Core Text descriptors on macOS and fontconfig on Linux and discovers fallback per missing codepoint; kitty uses Core Text / fontconfig per-codepoint fallback; Skia's `SkFontMgr::matchFamilyStyleCharacter` maps to `CTFontCreateForString` on macOS and to fontconfig charset matching on Linux. None of these do a full file scan at start-up. Verify in [alacritty/crossfont](https://github.com/alacritty/crossfont), [ghostty-org/ghostty src/font/discovery.zig](https://github.com/ghostty-org/ghostty), [kovidgoyal/kitty](https://github.com/kovidgoyal/kitty), [wezterm/wezterm](https://github.com/wezterm/wezterm) before relying on it.
- **Why `FcInitLoadConfigAndFonts` takes 15 ms or more:** not verified with a source this session. Background knowledge: fontconfig parses all XML config files under `/etc/fonts` and `conf.d` on every init (no cache for configuration), then mmaps one cache file per font directory from `/var/cache/fontconfig` or `~/.cache/fontconfig` (keyed by directory path hash and validated against directory mtime), and stats each directory. On a distro with many conf.d snippets and many font directories, XML parsing plus stat/mmap of dozens of cache files plausibly accounts for 15 to 35 ms. Confirm with `FC_DEBUG` and `strace -c` on the Linux laptop.
- Whether `CTFontCreateForString` cost is lower if called with a font created by `CTFontCreateUIFontForLanguage` versus a named font; not measured.
- Windows (DirectWrite `IDWriteFontFallback::MapCharacters`) not covered; out of stated scope.

## cosmic-text vs parley for an editor

### Takeaway
Both are active and current as of late 2026 (cosmic-text 0.19.0, April 2026; parley 0.12.0, October 2026), both shape with HarfRust and render via swash-family tooling. cosmic-text is the pragmatic choice for an editor already built on it (buffer/editor abstractions, used by COSMIC and Zed on Linux); parley has the cleaner font fallback architecture (fontique) and richer style-span layout, but is pre-1.0 with breaking releases and still does an eager full scan on macOS.

### Cited Findings
- cosmic-text 0.19.0 published 2026-04-22; 0.18.1 and 0.18.2 both on 2026-02-20; source: [GitHub releases](https://api.github.com/repos/pop-os/cosmic-text/releases)
- cosmic-text: HarfRust shaping, swash rendering (ligatures, colour emoji), custom bidi layout, full feature set on Linux, macOS, Windows; `no_std` incomplete; source: [docs.rs cosmic-text](https://docs.rs/crate/cosmic-text/latest)
- parley 0.12.0 published 2026-10-09, "provides an API for implementing rich text layout", HarfRust shaping, fontique 0.12; source: [docs.rs parley](https://docs.rs/crate/parley/latest)
- fontique moved to the Rust 2024 edition in July 2026 and has had several platform-backend fixes in 2026 (Core Text enumeration Feb, PingFang workaround Mar); source: [fontique coretext.rs commit history](https://api.github.com/repos/linebender/parley/commits?path=fontique/src/backend/coretext.rs)
- Zed uses cosmic-text for its Linux/wgpu text system; source: [Zed cosmic_text_system.rs](https://github.com/zed-industries/zed/blob/main/crates/gpui_wgpu/src/cosmic_text_system.rs)
- cosmic-text's maintainers are adding start-up performance features (persistent font index cache, June 2026); source: [cache.rs history](https://api.github.com/repos/pop-os/cosmic-text/commits?path=src/font/cache.rs)

### Inferences
- For vimmed, stay on cosmic-text and fix fallback via a custom `Fallback` plus OS-driven lazy loading (previous section). A parley migration would cost a layout rewrite and would not remove the macOS scan cost. Revisit parley if you need per-span font metrics (mixed sizes in one line) or better script-aware fallback across many languages.
- Track the next cosmic-text release for `new_cached`; it would replace your hand-rolled path cache if you ever want the full index, but a trimmed db remains faster than any full index because `new_with_locale_and_db_and_fallback` still iterates every face.

### Gaps
- No benchmark comparing cosmic-text and parley layout throughput for a 1,000-line monospace buffer was found. A head-to-head in the spike harness would be the only reliable data.
- Maintainer details (System76 team for cosmic-text, Linebender for parley) are from background knowledge; per-span metrics support in cosmic-text `Attrs` (per-span `Metrics` were added in 0.12-ish via `metrics_opt`) not verified this session.

## tree-sitter: why HighlightConfiguration::new is slow and how to start fast

### Takeaway
`HighlightConfiguration::new` compiles the highlights, injections and locals queries into a single `Query`, and query compilation (pattern analysis over every node type in large `.scm` files) is the dominant one-off cost. The established mitigations are: compile lazily per language on first use, cache the compiled configuration for the process (Helix uses `OnceLock` per language for each query kind), compile off the UI thread, and trim the query files. Highlighting after first paint, as you already do, is correct.

### Cited Findings
- tree-sitter-highlight 0.27.1 (2026-10-08) depends on tree-sitter `^0.27.1`. `HighlightConfiguration::new(language, name, highlights, injections, locals) -> Result<Self, QueryError>`; empty injections disables injection, empty locals disables local tracking. `configure(&mut self, names)` sets recognised highlight names by prefix matching. The struct is immutable after configuration and shareable across threads; source: [docs.rs HighlightConfiguration](https://docs.rs/tree-sitter-highlight/latest/tree_sitter_highlight/struct.HighlightConfiguration.html); [crates.io](https://crates.io/crates/tree-sitter-highlight)
- Emacs developers measured query object creation at about 10.4 ms out of 15.8 ms per font-lock call before moving to compile-once-and-reuse; source: [emacs-devel thread](https://yhetil.org/emacs-devel/87sfpekf6t.fsf@gmail.com/) (older material; tree-sitter C core, still relevant)
- nvim-treesitter's HTML highlight queries were found to be "extremely slow"; a Review Board rewrite of the HTML queries reported about 100x better performance; source: [Review Board r/14521](https://reviews.reviewboard.org/r/14521/)
- Helix stores each language's syntax config and auxiliary queries in `OnceLock<Option<...>>` fields (`syntax`, `indent_query`, `textobject_query`, `tag_query`, `rainbow_query`), so they are compiled on first use, not at start-up; source: [helix-core/src/syntax.rs](https://github.com/helix-editor/helix/blob/master/helix-core/src/syntax.rs)
- Helix now highlights via its own `tree-house` crate (0.3.0, "a robust and cozy highlighter library for tree-sitter", with a highlighter and query iterator that works across injections) rather than tree-sitter-highlight; source: [tree-house README](https://docs.rs/crate/tree-house/latest/source/README.md)
- Zed limits its highlight `QueryCursor` to a byte range (visible region) to fix slow highlighting on files with broken syntax; source: [Zed commit #52674](https://git.secluded.site/zed/commit/6cdf954e2ce0e1a6b83ce116b81258d3b512adf1)

### Inferences
- Concrete plan for vimmed:
  1. Keep a `HashMap<LanguageId, OnceLock<Arc<HighlightConfiguration>>>`; compile only when a buffer of that language is first opened.
  2. Compile on a worker thread (the config is `Send + Sync` once configured); first paint shows plain text, colour arrives when ready (no layout change, as in your spike).
  3. Once the first file is open, optionally precompile other likely languages (e.g. same project's languages detected from the file tree) at idle priority.
  4. Pass empty `locals` unless you need local-variable colouring; locals queries add compile and run cost. Pass empty `injections` for languages where you do not need embedded languages (keep them for Markdown, HTML, and possibly Bash heredocs).
  5. Restrict highlighting to the visible byte range plus a margin (`QueryCursor::set_byte_range`), as Zed does; with tree-sitter-highlight you can do this by feeding only the visible slice, or move to a custom `QueryCursor` loop over the tree.
  6. Trim or rewrite unusually heavy upstream query files (HTML is a known offender); measure each language's compile time once in a benchmark.
- tree-sitter `Query` objects cannot be serialised to disk in the public API, so there is no "cache compiled queries on disk" option; lazy plus background compile is the practical ceiling.
- **Alternatives:** tree-sitter-highlight remains the official crate and is fine for a single-buffer editor. Helix's tree-house is the strongest alternative for injection-heavy work. arborium (2.18.2, Aug 2026) bundles many grammars and queries; inkjet (0.11.1, last release Sept 2024) appears stale; two-face (0.5.2, Aug 2026) is syntect/bat syntax definitions, not tree-sitter, and syntect (5.3.0, Sept 2025) is the TextMate-regex route (no incremental parse tree, but no query compile step either); source: [crates.io arborium](https://crates.io/crates/arborium); [crates.io inkjet](https://crates.io/crates/inkjet); [crates.io two-face](https://crates.io/crates/two-face); [crates.io syntect](https://crates.io/crates/syntect)

### Gaps
- No primary source found that profiles *which part* of `Query::new` dominates (pattern parsing vs step analysis vs `ts_query__analyze_patterns`). Your measured 10 ms (Mac) and 32 to 37 ms (i7-7700HQ) for Rust are the best numbers available; a `perf` profile would confirm.
- Neovim, Lapce and difftastic start-up strategies were not verified this session. Background knowledge (unverified): Neovim parses queries lazily via `vim.treesitter.query.get` with memoisation; difftastic does not use highlight queries for diffing at all.
- Whether the newer tree-sitter 0.25+ query APIs (e.g. `QueryCursor` streaming iterators) change compile cost: not found.

## Grammar crate availability and binary size

### Takeaway
All ten languages have maintained crates on crates.io, mostly under the `tree-sitter` or `tree-sitter-grammars` organisations; SQL and Markdown need care in picking the right crate (`tree-sitter-sequel` and `tree-sitter-md`). Modern grammar crates depend on the small `tree-sitter-language` crate rather than a specific tree-sitter version, so they work with tree-sitter 0.27.

### Cited Findings
(crates.io API, queried 2026-11-03: latest stable, last updated, total downloads)
- `tree-sitter-rust` 0.24.2, 2026-03-27, 24.0M; source: [crates.io](https://crates.io/crates/tree-sitter-rust)
- `tree-sitter-go` 0.25.0, 2025-08-29, 15.9M; source: [crates.io](https://crates.io/crates/tree-sitter-go)
- `tree-sitter-javascript` 0.25.0, 2025-09-01, 16.7M; source: [crates.io](https://crates.io/crates/tree-sitter-javascript)
- `tree-sitter-typescript` 0.23.2, 2024-11-11, 18.9M (ships both TypeScript and TSX); source: [crates.io](https://crates.io/crates/tree-sitter-typescript)
- `tree-sitter-ruby` 0.23.1, 2024-11-11, 8.3M; source: [crates.io](https://crates.io/crates/tree-sitter-ruby)
- SQL: `tree-sitter-sequel` 0.3.11, 2025-10-01, 1.7M, repo derekstride/tree-sitter-sql (the maintained one); `tree-sitter-sql` 0.0.2 is a 2021 abandoned crate (m-novikov), avoid; source: [crates.io sequel](https://crates.io/crates/tree-sitter-sequel); [crates.io sql](https://crates.io/crates/tree-sitter-sql)
- `tree-sitter-html` 0.23.2, 2024-11-11, 5.3M; source: [crates.io](https://crates.io/crates/tree-sitter-html)
- `tree-sitter-css` 0.25.0, 2025-09-28, 5.2M; source: [crates.io](https://crates.io/crates/tree-sitter-css)
- `tree-sitter-bash` 0.25.1, 2025-12-02, 20.8M; source: [crates.io](https://crates.io/crates/tree-sitter-bash)
- `tree-sitter-hcl` 1.1.0, 2025-05-16, 3.1M, tree-sitter-grammars org (covers HCL generally; Terraform/OpenTofu use the same syntax); source: [crates.io](https://crates.io/crates/tree-sitter-hcl)
- Markdown: `tree-sitter-md` 0.5.4, 2026-10-24, 2.3M, tree-sitter-grammars/tree-sitter-markdown (two grammars: block and inline, which need injection to combine); `tree-sitter-markdown` 0.7.1 is a 2021 ikatyang crate, avoid; source: [crates.io md](https://crates.io/crates/tree-sitter-md); [crates.io markdown](https://crates.io/crates/tree-sitter-markdown)
- `tree-sitter-language` 0.1.9, 2026-10-08, the shared ABI shim crate; source: [crates.io](https://crates.io/crates/tree-sitter-language)

### Inferences
- Markdown and HTML are the two languages that need the injection machinery in practice (fenced code blocks, inline grammar, `<script>`/`<style>`), so design the highlight layer with injections from day one even if you start with the other eight.
- Check each crate ships `queries/highlights.scm` (most `tree-sitter`-org crates expose a `HIGHLIGHTS_QUERY` constant); where it is missing or weak (SQL, HCL often), vendor queries from Helix's `runtime/queries` or nvim-treesitter and record their licence.

### Gaps
- **Binary size of ten grammars:** not measured or sourced. The generated `parser.c` tables dominate; background knowledge suggests the large ones are TypeScript/TSX, Ruby, Bash and SQL at roughly 1 to 3 MB each of object code, and the total is likely in the 10 to 25 MB range before stripping. Measure with `cargo bloat --crates` on a release build of the spike; consider a dynamic grammar loading model (Helix's `helix-loader` loads grammars at runtime via `get_language`) if size matters; source: [helix-loader grammar.rs](https://github.com/helix-editor/helix/blob/master/helix-loader/src/grammar.rs)

## Incremental re-highlighting on edits (1,000-line file)

### Takeaway
The standard pattern is `tree.edit(&InputEdit)` followed by `parser.parse(text, Some(&old_tree))`, then re-running highlight queries only over `old_tree.changed_ranges(&new_tree)` intersected with the viewport. For a 1,000-line file, incremental re-parse is expected to be well under a millisecond, but I found no 2024+ primary benchmark to cite.

### Cited Findings
- Zed restricts the highlight query cursor to a byte range to keep highlighting cheap, which is the same idea applied to re-highlighting after edits; source: [Zed commit #52674](https://git.secluded.site/zed/commit/6cdf954e2ce0e1a6b83ce116b81258d3b512adf1)
- tree-sitter-highlight's `HighlightConfiguration` is immutable and reusable, so per-edit cost is parse plus query execution, not compilation; source: [docs.rs](https://docs.rs/tree-sitter-highlight/latest/tree_sitter_highlight/struct.HighlightConfiguration.html)

### Inferences
- tree-sitter-highlight's `Highlighter::highlight` API re-parses the source it is given from scratch (it does not take an old tree), so for incremental editing either highlight only the visible slice each frame or move to your own `Parser` + `Tree` + `QueryCursor` loop with `tree.edit` and `changed_ranges`. Helix (tree-house) and Zed both own the parse tree and run queries themselves for this reason.
- Run parse and query on a worker, keep last-known highlight spans, and shift them by the edit delta so colours do not flicker while the new result is pending.

### Gaps
- No sourced 2024+ timing for incremental `parse` on a 1,000-line Rust file. Add a micro-benchmark to the spike: time `tree.edit` + `parse(Some(&old))` for single-character inserts at the top, middle and end, plus `changed_ranges` and a viewport-limited `QueryCursor` run.
- tree-sitter-highlight's internal handling of old trees not confirmed from source this session.
