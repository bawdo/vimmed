# Crate maturity, packaging and single-source naming for the foo_edit native stack

Research date: 2026-11-03. Version and date figures below were pulled live from the crates.io API and GitHub API on that date unless flagged otherwise. Items marked "(prior knowledge, unverified this session)" come from background knowledge and were not confirmed against a live source; treat them as hypotheses.

## Q1. Crate status: versions, cadence, maintainers, bus factor, churn, known issues

### Takeaway
Every crate in the stack is alive and released in 2026, but they fall into two groups. Slow and stable: tiny-skia, image, yeslogic-fontconfig-sys, objc2 framework crates, swash. Fast and breaking: cosmic-text, parley, vello_cpu, tree-sitter, gtk4-rs (yearly). The biggest single-person risks are cosmic-text (one System76 engineer) and objc2 (one maintainer, but extremely active). Parley has a broader maintainer base than cosmic-text and is where the ecosystem (Bevy) is moving, but it churns just as fast.

### Cited Findings

**Release table (crates.io API, 2026-11-03)**

| Crate | Latest | Recent releases (date) | Downloads | MSRV |
|---|---|---|---|---|
| cosmic-text | 0.19.0 | 0.16.0 (2025-12-29), 0.17.0 (2026-01-29), 0.18.0 (2026-02-19), 0.19.0 (2026-04-22) | 11.5M | 1.89 |
| parley | 0.12.0 | 0.7.0 (2025-11-24), 0.8.0 (2026-03-27), 0.9.0 (2026-04-21), 0.10.0 (2026-06-01), 0.11.0 (2026-06-26), 0.12.0 (2026-10-09) | 5.2M | 1.88 |
| swash | 0.2.10 | 0.2.6 (2025-10-01), 0.2.7 (2026-03-27), 0.2.9 (2026-06-12), 0.2.10 (2026-07-17) | 17.6M | n/a |
| tiny-skia | 0.12.0 | 0.11.4 (2024-02-04), then 0.12.0 (2026-02-02) | 64.1M | none declared |
| vello_cpu | 0.3.0 | 0.0.5 (2026-01-08) ... 0.0.9 (2026-05-30), 0.1.0 (2026-07-29), 0.2.0 (2026-08-07), 0.3.0 (2026-10-02) | 9.6M | 1.89 |
| vello_hybrid | 0.2.0 | 0.1.0 (2026-07-29), 0.2.0 (2026-08-07) | 66k | n/a |
| tree-sitter / tree-sitter-highlight | 0.27.1 | 0.26.8 (2026-03-31), 0.26.9 to 0.26.13 (May to Aug 2026), 0.27.0 (2026-08-30), 0.27.1 (2026-10-08) | 52.5M / 4.6M | 1.90 |
| image | 0.25.10 | 0.25.1 (2024-03-31) ... 0.25.9 (2025-11-15), 0.25.10 (2026-03-10) | 236M | n/a |
| gtk4 | 0.11.5 | 0.11.0 (2026-02-20), 0.11.1 (03-14), 0.11.2 (04-05), 0.11.3 (04-27), 0.11.4 (06-29), 0.11.5 (09-20) | 5.3M | 1.92 |
| objc2 | 0.6.5 | 0.6.0 (2025-01-22), 0.6.1 (2025-04), 0.6.2 (2025-08), 0.6.3 (2025-10), 0.6.4 (2026-02-26), 0.6.5 and 0.5.3 (both 2026-10-05) | 151M | 1.71 |
| objc2-app-kit / -quartz-core / -io-surface | 0.3.2 | 0.3.0 (2025-01), 0.3.1 (2025-04-19), 0.3.2 (2025-10-04) | 81M / 80M / 63M | n/a |
| yeslogic-fontconfig-sys | 6.0.1 | 5.0.0 (2023-11), 6.0.0 (2024-07-03), 6.0.1 (2026-04-29) | 18.9M | n/a |
| smithay-client-toolkit | 0.21.1 | 0.19.0 (2024-06), 0.20.0 (2025-08-08), 0.21.0 (2026-07-20), 0.21.1 (2026-07-23) | 73.8M | n/a |
| cocoa (servo) | 0.27.0 | 0.26.0 (2024-08), 0.26.1 (2025-05), 0.27.0 (2026-09-07) | 36.8M | n/a |
| core-graphics (servo) | 0.25.0 | 0.25.0 (2025-05-27) | 92.6M | n/a |

All rows: [crates.io API](https://crates.io/api/v1/crates/cosmic-text) (same endpoint per crate name, e.g. [parley](https://crates.io/crates/parley), [tiny-skia](https://crates.io/crates/tiny-skia), [vello_cpu](https://crates.io/crates/vello_cpu), [gtk4](https://crates.io/crates/gtk4), [objc2](https://crates.io/crates/objc2), [tree-sitter](https://crates.io/crates/tree-sitter), [smithay-client-toolkit](https://crates.io/crates/smithay-client-toolkit)).

**Maintainer concentration (GitHub contributors API, top five by commits, 2026-11-03)**
- cosmic-text (pop-os/cosmic-text): jackpot51 477, hojjatabdollahi 61, hecrj 36, tigregalis 26, geieredgar 21. 2.2k stars, 126 open issues, pushed 2026-10-27. Effective bus factor of one (Jeremy Soller, System76). [GitHub API](https://api.github.com/repos/pop-os/cosmic-text/contributors)
- parley (linebender/parley): tomcur 165, waywardmonkeys 121, nicoburns 107, dfrg 52, DJMcNab 41. 773 stars, 122 open issues, pushed 2026-11-03. Spread across at least three active people. [GitHub API](https://api.github.com/repos/linebender/parley/contributors)
- tiny-skia (linebender/tiny-skia): RazrFalcon 246, waywardmonkeys 19, then single digits. Last push 2026-08-20, 51 open issues. [GitHub API](https://api.github.com/repos/linebender/tiny-skia/contributors)
- vello (linebender/vello, hosts vello_cpu and vello_hybrid): raphlinus 502, LaurenzV 313, dfrg 199, DJMcNab 171, armansito 171. 4.5k stars, pushed 2026-11-03. [GitHub API](https://api.github.com/repos/linebender/vello/contributors)
- swash (dfrg/swash): dfrg 101, waywardmonkeys 37, kirawi 14, nicoburns 13. Pushed 2026-07-17. [GitHub API](https://api.github.com/repos/dfrg/swash/contributors)
- objc2 (madsmtm/objc2): madsmtm 3237, SSheldon 1210 (original `objc` author), silvanshade 68. 1.1k stars, 111 open issues, pushed 2026-10-17. [GitHub API](https://api.github.com/repos/madsmtm/objc2/contributors)
- gtk4-rs: bilelmoussaoui 1533, Hofer-Julian 517, sdroege 502, sfanxiang 284. Pushed 2026-11-03. [GitHub API](https://api.github.com/repos/gtk-rs/gtk4-rs/contributors)
- smithay/client-toolkit: elinorbgr 192, i509VCB 140, kchibisov 67, ids1024 63, danieldg 58. Pushed 2026-10-16. [GitHub API](https://api.github.com/repos/smithay/client-toolkit/contributors)
- tree-sitter: maxbrunsfeld 3325, amaanq 764, WillLillis 461, ahlinc 353. 27k stars. [GitHub API](https://api.github.com/repos/tree-sitter/tree-sitter/contributors)

**cosmic-text**
- 0.19.0 depends on harfrust ^0.5.0 and skrifa ^0.40.0, fontdb ^0.23; swash ^0.2.6 and syntect are optional features. [crates.io deps](https://crates.io/api/v1/crates/cosmic-text/0.19.0/dependencies)
- Upstream harfrust is at 0.15.0 (2026-10-22) and skrifa at 0.49.0 (2026-10-21), so cosmic-text 0.19 is roughly ten harfrust minors and nine skrifa minors behind current. [harfrust](https://crates.io/crates/harfrust), [skrifa](https://crates.io/crates/skrifa)
- Changelog, 0.15.0 (2025-10-30): replaced rustybuzz with HarfRust, replaced peniko with linebender_resource_handle, added an alignment parameter to `set_text` (signature change), pixel-based `Editor` scrolling. 0.16.0: new `Renderer` trait and `Hinting` enum. 0.17.0: removed `Attrs::matches`, changed font fallback. 0.17.1: MSRV raised to 1.89 in a patch release. 0.17.2: `Motion::Home`/`End` semantics changed in a patch release. 0.19.0: buffer setters became lazy (changes when layout happens). [cosmic-text CHANGELOG](https://raw.githubusercontent.com/pop-os/cosmic-text/main/CHANGELOG.md)
- Breaking-ish behaviour changes landing in patch releases (0.17.1 MSRV, 0.17.2 motion semantics) means `~0.19.0` style pinning plus Cargo.lock is not enough on its own; review the changelog on every bump. [cosmic-text CHANGELOG](https://raw.githubusercontent.com/pop-os/cosmic-text/main/CHANGELOG.md)

**cosmic-text vs parley for an editor**
- Bevy has migrated `bevy_text` from cosmic-text to parley. [bevy PR #22879](https://github.com/bevyengine/bevy/pull/22879); discussion in [bevy issue #21765](https://github.com/bevyengine/bevy/issues/21765)
- A commentator in the Bevy discussion framed cosmic-text as aimed at a large single-buffer editor or terminal view and parley at rich text and input widgets (opinion, not an official position). [bevy issue #21765](https://github.com/bevyengine/bevy/issues/21765)
- One project's ADR benchmark (single machine, single font) found cosmic-text about 16% faster on a single-cluster hot path, but parley about 4.8x faster on full mixed lines, about 2x on full-screen throughput, about 2.7x on fallback-heavy text, and cosmic-text cold start vastly slower. They kept parley as default. [pragmatrix/massive PR #154](https://github.com/pragmatrix/massive/pull/154)
- An Arabic-text spike chose parley because of persistent cosmic-text BiDi wrap and cursive-disconnect bugs (single team's experience). [ofuqq/ruqa issue #4](https://github.com/ofuqq/ruqa/issues/4)
- A UI project moved caret, selection and hit-testing onto parley's layout and removed its cosmic-text caret code; the same PR notes parley's own editing helpers are still being built out. [vanyastaff/flui PR #1403](https://github.com/vanyastaff/flui/pull/1403)
- parley 0.12 is split into parley, parley_engine, parlance and fontique (0.12.0, released in lock-step). [crates.io deps](https://crates.io/api/v1/crates/parley/0.12.0/dependencies), [fontique](https://crates.io/crates/fontique)

**tiny-skia vs vello_cpu / vello_hybrid**
- tiny-skia now lives in the linebender GitHub org; README still says "this is not a Skia replacement and never will be. It's more of a research project." [linebender/tiny-skia](https://github.com/linebender/tiny-skia)
- Two-year gap between 0.11.4 (2024-02-04) and 0.12.0 (2026-02-02); repo last pushed 2026-08-20. Maintenance mode, not abandoned. [crates.io](https://crates.io/crates/tiny-skia), [GitHub API](https://api.github.com/repos/linebender/tiny-skia)
- tiny-skia 0.12 dependency set is tiny (arrayref, arrayvec, bytemuck, cfg-if, log, tiny-skia-path, optional png). [crates.io deps](https://crates.io/api/v1/crates/tiny-skia/0.12.0/dependencies)
- vello_cpu went 0.0.x to 0.1.0 (2026-07-29), 0.2.0 (2026-08-07), 0.3.0 (2026-10-02): three breaking releases in ten weeks. It optionally uses rayon for multithreading and `glifo` for glyphs. [crates.io](https://crates.io/crates/vello_cpu), [deps](https://crates.io/api/v1/crates/vello_cpu/0.3.0/dependencies)
- vello_hybrid has about 66k downloads versus vello_cpu's 9.6M, so it is much less exercised. [crates.io](https://crates.io/crates/vello_hybrid)

**swash**
- swash 0.2.10 requires skrifa `>=0.31.1, <=0.44`, while current skrifa is 0.49 and cosmic-text 0.19 wants skrifa ^0.40. A Cargo graph mixing swash with parley or newer fontations crates may therefore pull two skrifa versions. [swash deps](https://crates.io/api/v1/crates/swash/0.2.10/dependencies), [skrifa](https://crates.io/crates/skrifa)
- swash is a single-owner repo (dfrg) with four releases in 2026 (latest 2026-07-17). [crates.io](https://crates.io/crates/swash), [GitHub API](https://api.github.com/repos/dfrg/swash)

**objc2 ecosystem vs cocoa/core-graphics**
- docs.rs pages for `cocoa` and `cocoa-foundation` state they are deprecated and point to the objc2 crates (`objc2-foundation` as the Foundation replacement). [docs.rs cocoa](https://docs.rs/cocoa/latest), [cocoa-foundation source](https://docs.rs/crate/cocoa-foundation/latest/source/src/lib.rs)
- The servo core-foundation-rs README itself does not mention objc2 or deprecation, and `cocoa` 0.27.0 was still released 2026-09-07, so the deprecation is documented but the crates still receive maintenance releases. [servo/core-foundation-rs](https://github.com/servo/core-foundation-rs), [crates.io](https://crates.io/crates/cocoa)
- baseview (audio plugin windowing) merged a migration from `cocoa`, `core_foundation` and `objc` to objc2 on 2026-05-14, citing typed objects instead of `id` and less `unsafe`; tested on macOS 26.2. [RustAudio/baseview PR #240](https://github.com/RustAudio/baseview/pull/240)
- objc2 is a merge of `objc` (renamed objc2), `objc-foundation` and `block` (renamed block2), originally by SSheldon. [madsmtm/objc2](https://github.com/madsmtm/objc2)
- objc2-app-kit 0.3.2 accepts `objc2 >=0.6.2, <0.8.0` and `block2 >=0.6.1, <0.8.0`, indicating the framework crates are prepared to span the next objc2 major. [crates.io deps](https://crates.io/api/v1/crates/objc2-app-kit/0.3.2/dependencies)
- objc2 0.6.5 and 0.5.3 were released the same day (2026-10-05), consistent with a semver-compat bridge release. [crates.io](https://crates.io/crates/objc2)
- objc2 0.6.5 ships an `available!()` macro "for determining whether code is running on" a given OS version (source: `src/macros/available.rs`, CHANGELOG). [objc2 0.6.5 crate source](https://crates.io/crates/objc2/0.6.5)
- objc2-app-kit 0.3.2 generates `NSView::displayLinkWithTarget_selector` (also on NSScreen and NSWindow) as a plain `unsafe fn` with no compile-time availability gating. [objc2-app-kit 0.3.2 source, src/generated/NSView.rs](https://crates.io/crates/objc2-app-kit/0.3.2)

**gtk4-rs**
- gtk4 0.11.5 exposes features v4_2 through v4_24; gtk4-sys 0.11.3 declares `system-deps` baseline gtk4 >= 4.0.0, with each `v4_x` feature raising the pkg-config requirement. [gtk4 crate](https://crates.io/crates/gtk4), [gtk4-sys 0.11.3 Cargo.toml](https://crates.io/crates/gtk4-sys/0.11.3)
- gtk4 0.11.5 declares MSRV 1.92. [crates.io](https://crates.io/crates/gtk4)
- gtk4-rs majors track the GNOME cycle: the 0.9 post (2024-07-17) says it was released "just in time for also being included in the GNOME 47 release". The gtk-rs blog had no 0.10 or 0.11 post visible when fetched. [gtk-rs blog](https://gtk-rs.org/blog/)
- 0.11.x point releases have come roughly every one to three months since 2026-02-20. [crates.io](https://crates.io/crates/gtk4)

**tree-sitter**
- Library supports ABI 15 with minimum compatible ABI 13; backwards compatible with older-generated grammars, not forwards. ABI 15 came in 0.25.0 (early 2025). [docs.rs api.h](https://docs.rs/crate/tree-sitter/latest/source/include/tree_sitter/api.h), [openSUSE changes](https://build.opensuse.org/projects/home:llyyr/packages/tree-sitter/files/tree-sitter.changes?expand=0)
- 0.27.0 breaking changes (Rust): `Node::child_count` returns `u32`, parser loggers must be `Send + 'static`, query iterators tied to options lifetime, `ts_set_allocator` takes one `Allocator`. tree-sitter-highlight: UTF-16 support, error enums now include `LanguageError`. Edition 2024, MSRV 1.90. No new ABI. [tree-sitter v0.27.0 release](https://github.com/tree-sitter/tree-sitter/releases/tag/v0.27.0)
- Patch releases roughly monthly through 2026. [crates.io](https://crates.io/crates/tree-sitter)

**smithay-client-toolkit**
- One breaking release per year recently (0.19 2024-06, 0.20 2025-08, 0.21 2026-07). 0.21 depends on `wayland-protocols-experimental ^20251230`, wayland-client 0.31, optional calloop 0.14. [crates.io](https://crates.io/crates/smithay-client-toolkit), [deps](https://crates.io/api/v1/crates/smithay-client-toolkit/0.21.1/dependencies)

### Inferences
- **Text layout recommendation:** for a markdown editor (proportional prose, headings, inline code, mixed scripts, not a monospace terminal) parley is the better long-term bet: broader maintainers, ecosystem momentum, on current fontations/harfrust. cosmic-text is the lower-effort start because it ships an `Editor` and buffer model, but it trails upstream shaping crates and depends on one person. If the spike already uses cosmic-text 0.19, keep a thin internal `TextLayout` trait so swapping to parley stays cheap. Either way, budget a breaking upgrade every one to three months.
- **Raster recommendation:** tiny-skia 0.12 is the safe, stable, low-churn choice for a CPU renderer today. vello_cpu is the future direction from the same org and likely faster (multithreaded), but at 0.3 with three majors in ten weeks it is a moving target. Keep rasterisation behind an internal trait; revisit vello_cpu when it slows down.
- **Glyph rasterisation:** swash works but is pinned to an older skrifa window. If moving to parley, check whether parley/vello's own glyph path (skrifa outlines into the raster backend) removes the need for swash, avoiding duplicate skrifa versions.
- **objc2 is the de facto standard** for new macOS Rust code; `cocoa`/`core-graphics` are documented as deprecated and should not be used for new work. The single-maintainer risk is mitigated by the code being mostly generated from Apple SDK headers.
- **`NSView.displayLink` (macOS 14+)**: because objc2 generates it without availability gating, calling it on macOS 13 would fail at runtime (unrecognised selector). Wrap in `available!(macos = 14.0)` with a CVDisplayLink or timer fallback, or set the minimum deployment target to 14. CVDisplayLink being deprecated in macOS 14 is prior knowledge, unverified this session. IOSurface has existed since macOS 10.6 (prior knowledge), so it imposes no meaningful floor.
- **gtk4 MSRV 1.92** is high relative to distro toolchains; this matters for Debian/Ubuntu archive builds (see Q3).

### Gaps
- No live source fetched for cosmic-text's or parley's roadmap posts (Linebender blog, System76 blog); cosmic-text's planned harfrust/skrifa catch-up is unknown.
- Did not confirm whether tiny-skia has an official "maintenance mode" statement from Linebender; status inferred from release gap and org move.
- No numerical comparison of tiny-skia vs vello_cpu performance was retrieved.
- gtk4-rs 0.10 release date and official 0.11 release post not retrieved; could not confirm whether 0.12 is planned for GNOME 50 timing.
- Notable users beyond Bevy (parley), COSMIC desktop (cosmic-text, implied by ownership), Zed/Helix/Neovim (tree-sitter, prior knowledge) were not verified this session.

## Q2. AI-assisted coding and API churn

### Takeaway
No direct empirical evidence was found on how well coding models handle these crates. The proxy is churn: crates with breaking releases in the last six months (cosmic-text, parley, vello_cpu, tree-sitter 0.27, gtk4 0.11, sctk 0.21) will be badly served by model knowledge with a mid-2025 or earlier cutoff; stable ones (tiny-skia, image 0.25, objc2 0.6/0.3) will be served well.

### Cited Findings
- cosmic-text: five minor (breaking) releases between 2025-10-30 and 2026-04-22, including renamed dependency types (peniko to linebender_resource_handle) and changed method signatures. [cosmic-text CHANGELOG](https://raw.githubusercontent.com/pop-os/cosmic-text/main/CHANGELOG.md)
- parley: six minor releases from 0.7.0 (2025-11-24) to 0.12.0 (2026-10-09). [crates.io](https://crates.io/crates/parley)
- vello_cpu: 0.1, 0.2, 0.3 within 2026-07-29 to 2026-10-02. [crates.io](https://crates.io/crates/vello_cpu)
- tree-sitter 0.27.0 changed common Rust signatures (`child_count` to `u32`, logger bounds). [v0.27.0 release](https://github.com/tree-sitter/tree-sitter/releases/tag/v0.27.0)
- image has stayed on 0.25.x since 2024-03-31; tiny-skia 0.11 API was stable 2023 to 2026; objc2 0.6 since 2025-01 and framework crates 0.3 since 2025-01. [crates.io image](https://crates.io/crates/image), [tiny-skia](https://crates.io/crates/tiny-skia), [objc2](https://crates.io/crates/objc2)

### Inferences
- objc2 has an extra advantage for AI-assisted work: its API names are mechanically derived from Apple selectors (`displayLinkWithTarget_selector`), so a model that knows AppKit can predict the Rust name even if it has never seen the crate version.
- gtk4-rs patterns (subclassing macros, `glib::clone!` syntax) have changed across 0.7 to 0.11 (prior knowledge, unverified this session), so expect models to emit outdated `clone!` syntax.
- Practical mitigation: vendor `cargo doc` output or pin docs.rs links for the exact versions into agent context; keep each crate behind a thin internal module so churn is fixed in one place.

### Gaps
- No studies or benchmarks on LLM accuracy per Rust crate were found.

## Q3. Packaging: tools, macOS bundling/signing, distro policies, Flatpak

### Takeaway
Use cargo-deb and cargo-generate-rpm for .deb/.rpm artefacts (both active), a hand-rolled or xtask-driven .app assembly (the pattern Alacritty and WezTerm use) rather than depending on a bundler, Apple's `codesign`/`notarytool` on a Mac for signing (rcodesign only if signing from Linux CI). AUR conventions are well documented. Official Debian/Fedora archive inclusion is a separate, much harder project because they forbid or discourage vendored crates. Skip Flatpak for a $EDITOR-style tool.

### Cited Findings

**Tool status (crates.io and GitHub API, 2026-11-03)**
- cargo-deb 3.8.0 (2026-09-01), kornelski, frequent releases. [crates.io](https://crates.io/crates/cargo-deb)
- cargo-generate-rpm 0.21.1 (2026-10-17), cat-in-136, several releases a year. [crates.io](https://crates.io/crates/cargo-generate-rpm)
- cargo-packager 0.11.8 (2025-11-27), CrabNebula (Tauri-adjacent); no release in a year, but repo pushed 2026-11-02. [crates.io](https://crates.io/crates/cargo-packager), [GitHub API](https://api.github.com/repos/crabnebula-dev/cargo-packager)
- cargo-bundle revived: 0.6.0 (2022), 0.6.1 (2024-08), then 0.7.0 (2025-06) through 0.12.0 (2026-09-20). [crates.io](https://crates.io/crates/cargo-bundle)
- cargo-dist 0.34.0 (2026-11-02), axodotdev, active. [crates.io](https://crates.io/crates/cargo-dist)
- apple-codesign (rcodesign) 0.29.0 (2024-11-29), no crate release in about two years, though the apple-platform-rs repo was pushed 2026-10-17 (120 open issues). [crates.io](https://crates.io/crates/apple-codesign), [GitHub API](https://api.github.com/repos/indygreg/apple-platform-rs)
- rcodesign: Linux gnu builds need glibc 2.35+, musl builds lack PKCS#11; notarisation needs an App Store Connect API key (Issuer ID, Key ID, ECDSA private key), bundled via `encode-app-store-connect-api-key`; `Developer` role "should be sufficient". [apple-codesign getting started](https://gregoryszorc.com/docs/apple-codesign/main/apple_codesign_getting_started.html)

**How real projects build the .app**
- Zed: reads channel from a `RELEASE_CHANNEL` file, uses `sed` to rewrite `package.metadata.bundle-${channel}` to `package.metadata.bundle` in `crates/zed/Cargo.toml`, runs a forked `cargo-bundle` (`v0.6.1-zed`), then restores the file. Signs with `codesign`, notarises with `notarytool submit --wait` and `stapler staple`, falls back to ad-hoc signing without secrets. Builds per-arch DMGs (no `lipo`). [zed script/bundle-mac](https://raw.githubusercontent.com/zed-industries/zed/main/script/bundle-mac)
- WezTerm: builds both arches and runs `lipo ... -create` into `WezTerm.app/Contents/MacOS/`, signs with `codesign --options runtime --entitlements ... --deep`, notarises with `xcrun notarytool submit --wait`. Linux .desktop/appdata/icons installed under reverse-DNS `org.wezfurlong.wezterm.*` names. [wezterm ci/deploy.sh](https://raw.githubusercontent.com/wezterm/wezterm/main/ci/deploy.sh)
- Alacritty: Makefile copies a template `extra/osx/Alacritty.app` and drops the binary into `Contents/MacOS`; supports `binary-universal`; sets `MACOSX_DEPLOYMENT_TARGET="10.12"`. [alacritty Makefile](https://raw.githubusercontent.com/alacritty/alacritty/master/Makefile)

**Arch / AUR**
- Package name should normally match the binary name, all lowercase; for multi-binary projects the upstream crate name is usual. [Arch Rust package guidelines](https://manual.archlinux.page/package-guidelines/rust/)
- `makedepends=(cargo)`; depends usually just glibc/libgcc plus linked system libs (here gtk4, fontconfig). [Arch Rust package guidelines](https://manual.archlinux.page/package-guidelines/rust/)
- prepare: `export RUSTUP_TOOLCHAIN=stable; cargo fetch --locked --target host-tuple`. build: `export RUSTUP_TOOLCHAIN=stable; export CARGO_TARGET_DIR=target; cargo build --frozen --release --all-features`. `--frozen` equals `--locked --offline`. A `rust-toolchain.toml` in the source makes `RUSTUP_TOOLCHAIN` unnecessary. [Arch Rust package guidelines](https://manual.archlinux.page/package-guidelines/rust/)
- The Arch wiki page has moved to manual.archlinux.page; the wiki now redirects there. [redirect observed](https://wiki.archlinux.org/title/Rust_package_guidelines)

**Debian**
- Debian does not ship compiled Rust libraries (no stable ABI); library crates are packaged as source, and application packages build-depend on those library packages matching Cargo version constraints. All bin targets of a crate go into one binary package; tooling is debcargo and dh-cargo. [Debian Rust Team policy](https://rust-team.pages.debian.net/book/policy.html)
- Debian 12 ships GTK 4.8.3, Debian 13 ships 4.18.6 (see Q6). [packages.debian.org](https://packages.debian.org/search?keywords=libgtk-4-1&searchon=names&exact=1&suite=all&section=all)

**Fedora**
- A rust2rpm issue states bundling crate dependencies "is already forbidden for Fedora packages, unless it's impossible to build packages otherwise", and that rust-packaging v25 added support for building against vendored dependencies via `%cargo_prep`. [rust2rpm issue #139](https://pagure.io/fedora-rust/rust2rpm/issue/139)
- cargo-vendor-filterer is packaged in Fedora 42 and Rawhide. [Fedora Packages](https://packages.fedoraproject.org/pkgs/rust-cargo-vendor-filterer/cargo-vendor-filterer/fedora-42.html)

**Flatpak**
- `flatpak-spawn --host` runs a command unsandboxed on the host and needs the `org.freedesktop.Flatpak` D-Bus talk permission; `--sandbox-expose` only accepts names inside the app's sandbox directory. [flatpak-spawn(1)](https://man7.org/linux/man-pages/man1/flatpak-spawn.1.html)
- FileZilla's Flatpak broke custom external editors: host-spawn workarounds failed when the editor itself was a Flatpak (could not see the temp dir), and configured editor binaries were not found. [flathub FileZilla #40](https://github.com/flathub/org.filezillaproject.Filezilla/issues/40)

### Inferences
- **Recommended pipeline:** an `xtask` (or `just` recipe calling it) that (1) builds release binaries, (2) renders templates, (3) on macOS assembles `Name.app/Contents/{MacOS,Resources,Info.plist}`, `lipo`s aarch64 and x86_64 into a universal binary, signs with `codesign --options runtime` (hardened runtime is required for notarisation, prior knowledge), notarises with `notarytool`, staples, and builds a DMG; (4) on Linux calls `cargo deb` and `cargo generate-rpm` with generated metadata. Avoid depending on cargo-bundle or cargo-packager for the .app because both read static `[package.metadata]` tables, which fights the single-name requirement (Zed has to `sed` Cargo.toml for this reason).
- **CLI on PATH (macOS):** ship the launcher inside `Contents/MacOS/` (or `Contents/Resources/bin/`) and offer an "Install command line tool" action or Homebrew cask `binary` stanza that symlinks it into `/usr/local/bin` or `/opt/homebrew/bin` (prior knowledge pattern used by VS Code `code` and Zed `zed`; unverified this session).
- **rcodesign** is only worth it if signing must happen on Linux CI; the crate's two-year release gap is a risk. Prefer Apple's own tools on a macOS runner.
- **AUR:** ship `foo-edit` (from release tarball) and `foo-edit-git`. Keep `options=(!lto)` only if the project's own `[profile.release]` sets LTO and makepkg's LTO flags conflict; the fetched Arch text did not mention `!lto`, so that convention could not be confirmed this session. Commit `Cargo.lock` and a `rust-toolchain.toml`.
- **Debian/Fedora official archives:** with gtk4 0.11 needing MSRV 1.92 and fast-moving deps (cosmic-text/parley/vello) that are unlikely to be packaged at matching versions, official archive inclusion is impractical near-term. Ship your own .deb/.rpm (cargo-deb, cargo-generate-rpm) built against the oldest supported distro in a container, or via COPR/OBS/PPA where vendoring is permitted in practice.
- **Flatpak:** an editor used as `$EDITOR`/`git commit` target needs to open arbitrary paths synchronously, be invoked from a host shell, and block until close. Inside Flatpak, paths arrive via the document portal (`/run/user/.../doc/...` FUSE paths, prior knowledge), the `flatpak run` invocation is not a natural `$EDITOR`, and most editors that ship on Flathub request `--filesystem=host`, defeating the sandbox. Recommend: Flatpak optional and low priority, explicitly documented as "not for $EDITOR use".

### Gaps
- Fedora packaging guidelines page and its source were blocked (Anubis bot protection) or 404; the current official vendoring rule text was not retrieved. Only the rust2rpm issue was available.
- Debian's explicit policy on vendored crates for leaf applications was not found in the fetched policy page.
- Flatpak official docs on the document portal and `--filesystem=host` guidance were not fetched.
- cargo-packager, cargo-dist and tauri-bundler feature details (Info.plist templating, Linux formats) were not fetched; only release status.
- Universal-binary support in rcodesign not confirmed from the page fetched.

## Q4. Two binaries (libc-only launcher plus GTK window binary) vs one

### Takeaway
No packaging norm forbids it, and both cargo-deb and cargo-generate-rpm handle multiple binaries in one package. The friction is in naming (two derived names), macOS bundle layout, and keeping both binaries version-locked.

### Cited Findings
- Debian Rust policy puts all binary targets of a crate into one Debian binary package. [Debian Rust Team policy](https://rust-team.pages.debian.net/book/policy.html)
- Arch: for projects with more than one binary, the upstream crate name is the usual package name. [Arch Rust package guidelines](https://manual.archlinux.page/package-guidelines/rust/)
- Alacritty ships `alacritty` plus an `alacritty-msg` man page from one package, suggesting multi-tool packages are normal. [alacritty Makefile](https://raw.githubusercontent.com/alacritty/alacritty/master/Makefile)

### Inferences
- Two binaries are fine for .deb/.rpm/AUR: one package, both in `/usr/bin` (or the GTK binary under `/usr/libexec/<name>/` or `/usr/lib/<name>/` so only the launcher is on PATH; libexec use is a common FHS pattern, prior knowledge).
- On macOS, the launcher can live in `Contents/MacOS/` next to the main executable named in `CFBundleExecutable`; the CLI symlink points at the launcher. Both must be signed (inner binaries first, then the bundle).
- The launcher must find the GUI binary relative to its own `current_exe()` rather than via PATH, so renames never break the lookup.
- Version skew: launcher and GUI should exchange a protocol version (embed `CARGO_PKG_VERSION`) and refuse mismatches.
- Benefit of the split: the launcher stays libc-only so `$EDITOR` start-up is fast and works without a display; package `depends` still has to list gtk4 since the package as a whole needs it.

### Gaps
- No source found documenting distro reviewer objections to libexec helper binaries for Rust apps specifically.

## Q5. Single build-time name definition

### Takeaway
Cargo cannot compute `[[bin]] name`, so the standard workaround is: keep fixed internal binary names in Cargo, derive every user-visible name from one metadata value at build time (build.rs emitting `rustc-env`), and rename/assemble at an xtask packaging step that also renders Info.plist, .desktop, PKGBUILD, spec and cargo-deb metadata from templates.

### Cited Findings
- Cargo issue #1706 "Ability to specify the output name for a bin target different from the crate name" is closed. [rust-lang/cargo #1706](https://github.com/rust-lang/cargo/issues/1706)
- Zed keeps per-channel bundle metadata as `package.metadata.bundle-<channel>` tables and swaps them in with `sed` before running cargo-bundle, driven by a single `RELEASE_CHANNEL` file. [zed bundle-mac](https://raw.githubusercontent.com/zed-industries/zed/main/script/bundle-mac)
- Alacritty centralises names as Makefile variables (`TARGET = alacritty`, `APP_NAME = Alacritty.app`, `DMG_NAME = Alacritty.dmg`) and copies a checked-in .app template. [alacritty Makefile](https://raw.githubusercontent.com/alacritty/alacritty/master/Makefile)
- WezTerm hard-codes `WezTerm.app` and `org.wezfurlong.wezterm` across scripts rather than templating from one source. [wezterm ci/deploy.sh](https://raw.githubusercontent.com/wezterm/wezterm/main/ci/deploy.sh)

### Inferences
- Cargo #1706 was closed in favour of the unstable `different-binary-name` feature (`filename = "..."` in `[[bin]]`, nightly `-Z`) per prior knowledge, unverified this session. Even if stabilised, `filename` is still a static string, so it does not solve "compute from one definition".
- **Recommended pattern for foo_edit:**
  1. One source of truth: `[workspace.metadata.app]` in the root Cargo.toml (or a tiny `app.toml`) with `slug = "foo-edit"`, `display = "Foo Edit"`, `reverse_dns = "org.example.FooEdit"`. Derive variants (binary name, GTK binary `<slug>-gtk`, config dir, desktop file id) by rule in code, not by listing them.
  2. A shared `build.rs` (or a small `app-name` crate with a build script) reads that file and emits `cargo:rustc-env=APP_SLUG=...`, `APP_DISPLAY=...`, `APP_ID=...` plus `cargo:rerun-if-changed`. Code uses `env!("APP_SLUG")` for window title, `--version`, usage text (clap `.name(env!(...))`), and config/state paths (`directories`/XDG and `~/Library/Application Support/<APP_ID>`).
  3. Cargo `[[bin]]` names stay neutral and fixed (e.g. `launcher`, `window`). The xtask copies `target/release/launcher` to `dist/<slug>` and `window` to `<slug>-gtk`, then renders `Info.plist`, `<APP_ID>.desktop`, AppStream metainfo, man page, PKGBUILD, RPM spec and cargo-deb `assets` from templates using the same values.
  4. Alternatively make the package name itself the slug (`name = "foo-edit"` with default `src/main.rs`), so `CARGO_PKG_NAME` and the default bin name both follow one edit. This works for one binary only; the second still needs a rule-based rename.
  5. Rename test: add a CI job that sets the slug to something else and greps the built artefacts for the old name.
- cargo-deb and cargo-generate-rpm read `[package.metadata.deb]` / `[package.metadata.generate-rpm]`; generate those tables (or pass overrides on the CLI) from the xtask so they never hard-code the name.
- `just`/`cargo-make` are fine front doors, but keep the logic in Rust (xtask) so templating and renames are testable and cross-platform.

### Gaps
- Did not verify cargo-deb's CLI override flags for package name or asset paths this session.
- Current stabilisation status of Cargo's `different-binary-name` not confirmed.

## Q6. Minimum supported versions: GTK4 on target distros, gtk4-rs feature flags, macOS floor

### Takeaway
GTK floor across the stated targets is set by Debian 12 (4.8) and Ubuntu 24.04 (4.14). gtk4-rs 0.11 builds against GTK 4.0+ by default, so enabling at most `v4_14` keeps Ubuntu 24.04, or `v4_8` keeps Debian 12. The Rust toolchain (gtk4 MSRV 1.92) is a bigger constraint for in-archive builds than GTK itself. On macOS, `NSView.displayLink` sets a 14.0 floor unless guarded with `available!`.

### Cited Findings
| Distro | GTK4 | Source |
|---|---|---|
| Debian 12 bookworm (oldstable) | 4.8.3+ds-2+deb12u1 | [packages.debian.org](https://packages.debian.org/search?keywords=libgtk-4-1&searchon=names&exact=1&suite=all&section=all) |
| Debian 13 trixie (stable) | 4.18.6+ds-2 | [packages.debian.org](https://packages.debian.org/source/stable/gtk4) |
| Debian forky/sid | 4.24.1+ds-1 | [Debian tracker](https://tracker.debian.org/pkg/gtk4) |
| Ubuntu 24.04 noble | 4.14.2+ds-1ubuntu1 (noble-updates 4.14.5) | [packages.ubuntu.com](https://packages.ubuntu.com/search?keywords=libgtk-4-1&searchon=names&exact=1&suite=all&section=all) |
| Ubuntu 26.04 resolute | 4.22.2+ds-1ubuntu1 (resolute-updates 4.22.4) | [packages.ubuntu.com](https://packages.ubuntu.com/search?keywords=libgtk-4-1&searchon=names&exact=1&suite=all&section=all) |
| Fedora 43 | 4.20.2-1.fc43 | [Fedora mdapi](https://mdapi.fedoraproject.org/f43/pkg/gtk4) |
| Fedora 44 | 4.22.5-2.fc44 | [Fedora mdapi](https://mdapi.fedoraproject.org/f44/pkg/gtk4) |
| Fedora 42 | no response from mdapi (likely retired branch) | [Fedora mdapi](https://mdapi.fedoraproject.org/f42/pkg/gtk4) |

- gtk4-sys 0.11.3 baseline `gtk4 >= 4.0.0`; each `v4_N` feature raises the pkg-config minimum to 4.N. gtk4 0.11.5 offers up to `v4_24`. [gtk4-sys 0.11.3](https://crates.io/crates/gtk4-sys/0.11.3), [gtk4](https://crates.io/crates/gtk4)
- gtk4 0.11.5 MSRV 1.92; tree-sitter 0.27 MSRV 1.90; cosmic-text 0.19 and vello_cpu 0.3 MSRV 1.89; parley 0.12 MSRV 1.88; objc2 0.6.5 MSRV 1.71. [crates.io](https://crates.io/crates/gtk4)
- `available!()` exists in objc2 0.6.x for runtime OS version checks; `displayLinkWithTarget_selector` is ungated in objc2-app-kit 0.3.2. [objc2 0.6.5](https://crates.io/crates/objc2/0.6.5), [objc2-app-kit 0.3.2](https://crates.io/crates/objc2-app-kit/0.3.2)
- Alacritty targets `MACOSX_DEPLOYMENT_TARGET=10.12`, showing how far back a Rust app can go when it avoids newer APIs. [alacritty Makefile](https://raw.githubusercontent.com/alacritty/alacritty/master/Makefile)

### Inferences
- Fedora 42 was released April 2025 and, on the usual 13-month cycle, reached end of life around mid-2026 (prior knowledge, consistent with mdapi returning nothing); drop it from the target list and use Fedora 43/44. Fedora 42 shipped GTK 4.18 (prior knowledge, unverified).
- **Recommended GTK floor:** `v4_14` (Ubuntu 24.04 LTS). Debian 12 is oldstable with 4.8; supporting it costs features such as `GtkGraphicsOffload` (4.14) and newer `GdkTexture` builders (prior knowledge of which features land in which version, unverified). If Debian 12 matters, gate those calls behind a cargo feature and runtime `gtk::check_version`.
- **Rust toolchain:** distro-archived rustc versions lag (Debian 13 ships around rustc 1.85, prior knowledge, unverified); with MSRV 1.92 the project must be built with rustup-provided toolchains for its own .deb/.rpm, not the archive compiler. Pin a `rust-toolchain.toml`.
- **macOS floor:** if the renderer uses `NSView.displayLink` unconditionally, set `LSMinimumSystemVersion` and `MACOSX_DEPLOYMENT_TARGET` to 14.0; otherwise use `available!(macos = 14.0)` with a fallback and target 12 or 13. With macOS 26 current, a 14.0 floor covers the last three releases and is a reasonable, simpler choice.

### Gaps
- Fedora 42 GTK version not retrieved live.
- Exact GTK version where each relevant API (graphics offload, `GdkMemoryTextureBuilder`, `GdkDmabufTextureBuilder`) landed not verified this session; check the gtk4-rs docs `v4_x` gates before choosing the floor.
- rustc versions in Debian 13, Ubuntu 24.04/26.04 and Fedora 43 archives not fetched.
