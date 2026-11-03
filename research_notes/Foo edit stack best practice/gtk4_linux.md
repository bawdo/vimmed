# GTK4 on Linux: best practice for a custom-drawn, low-latency editor widget (gtk4-rs 0.11, GTK 4.24)

Research date: 2026-11-03. Primary sources: GTK `NEWS` (main branch, latest entry 4.24.1, 01-10-2026), GTK source at commit `723ba236` (main, 3 Nov 2026), docs.gtk.org, Ghostty source at commit `0d1d5e88` (main, 3 Nov 2026), Ghostty discussions, Repology. Where I read GTK or Ghostty source directly, the citation points at the file on GitHub's mirror (paths are as of those commits; line numbers can drift).

## Custom-drawn widgets: snapshot vs DrawingArea/cairo, texture types, and how GSK caches uploads

### Takeaway
Our spike's direction (custom `GtkWidget::snapshot()` appending `GdkMemoryTexture`s, one per strip, rebuilding only damaged strips) is the right shape for CPU-rasterised content in GTK 4.24, and the source confirms why it wins: the GPU renderers cache the uploaded image per `GdkTexture` object, and they never do partial uploads, so the only way to upload less per keystroke is to create smaller new textures. `GdkMemoryTextureBuilder`'s `update_texture`/`update_region` (4.16+) only feeds damage tracking (which region of the window to repaint), not upload size. Ghostty's current GTK frontend has moved off `GtkGLArea` to exactly this "build a `GdkTexture` per frame and hand it to snapshot" model (dmabuf, with a memory-texture fallback), which is strong prior art.

### Cited Findings
- `GdkMemoryTextureBuilder` exists since GTK 4.16 and "can be used for quick one-shot construction of textures as well as kept around and reused to construct multiple textures"; `set_update_texture` "Sets the texture to be updated by this texture" and `set_update_region` "Sets the region to be updated by this texture" (both 4.16) — [docs.gtk.org MemoryTextureBuilder](https://docs.gtk.org/gdk4/class.MemoryTextureBuilder.html), [set_update_region](https://docs.gtk.org/gdk4/method.MemoryTextureBuilder.set_update_region.html)
- `GdkMemoryTextureBuilder` landed in 4.15.4 (dev cycle for 4.16) — [GTK NEWS, 4.15.4](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- The docs describe the update region as being for cases like animations of large textures where consecutive textures only change in parts, so GTK can avoid re-rendering parts that did not change (example: a screen recording where only the pointer moves) — [docs.gtk.org set_update_region](https://docs.gtk.org/gdk4/method.MemoryTextureBuilder.set_update_region.html); pgi-docs labels the class "New in 4.20", which conflicts with the C docs and looks wrong — [pgi-docs](https://lazka.github.io/pgi-docs/Gdk-4.0/classes/MemoryTextureBuilder.html)
- Source: when `update_texture` is set, `gdk_memory_texture_new_from_builder` clips the update region to the texture and calls `gdk_texture_set_diff()`, linking the new texture into a chain with its predecessor. The same happens for `GdkGLTextureBuilder` and `GdkDmabufTextureBuilder` — [gdk/gdkmemorytexture.c ~L127-155](https://github.com/GNOME/gtk/blob/main/gdk/gdkmemorytexture.c), [gdk/gdkgltexture.c ~L262-290](https://github.com/GNOME/gtk/blob/main/gdk/gdkgltexture.c), [gdk/gdkdmabuftexture.c ~L200-298](https://github.com/GNOME/gtk/blob/main/gdk/gdkdmabuftexture.c)
- Source: the only consumer of that chain is `gdk_texture_diff()`, called from the texture node and texture-scale node diff functions (`gsktexturenode.c`, `gsktexturescalenode.c`). If two textures are not in the same chain, the diff is the whole texture rectangle. Nothing under `gsk/gpu/` reads the chain, so there is no partial upload path — [gdk/gdktexture.c ~L860-910](https://github.com/GNOME/gtk/blob/main/gdk/gdktexture.c), [gsk/gsktexturenode.c ~L199](https://github.com/GNOME/gtk/blob/main/gsk/gsktexturenode.c) (my grep of `gsk/gpu` for `gdk_texture_diff`/`previous_texture`/region uploads returned nothing)
- Source: the GPU cache stores uploaded images as per-texture "render data" (`gdk_texture_get_render_data`/`gdk_texture_set_render_data` in `gsk_gpu_cache_lookup_texture_image`/`gsk_gpu_cache_cache_texture_image`), so an unchanged `GdkTexture` object reused across frames is not re-uploaded, and a new object is uploaded in full — [gsk/gpu/gskgpucache.c ~L306-330, L556-582](https://github.com/GNOME/gtk/blob/main/gsk/gpu/gskgpucache.c)
- Third-party confirmation of the same caching behaviour: a browser project wrapped each frame in a `GdkMemoryTexture` so that GSK keeps the uploaded texture while the frame is current and redraws without a new frame cost no upload — [nordstjernen-browser PR #169](https://github.com/nordstjernen-web/nordstjernen-browser/pull/169)
- Old GL renderer gained update-region support for GL textures in 4.11.2 (2023), the precursor to the builders — [GTK NEWS, 4.11.2](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- `GdkDmabufTextureBuilder` added in 4.13.2; dmabuf import, graphics offload to Wayland subsurfaces via dmabuf textures from 4.13.3 onward; "Make dmabufs work without GL" in 4.17.0 — [GTK NEWS](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- GSK (4.23.0, shipped in 4.24): "The cache allocator has been rewritten" (!9533 "Add a new atlas allocator and rework caching APIs", Benjamin Otte) and "We are now using clip masks to reduce the number of offscreens" — [GTK NEWS, 4.23.0](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- Matthias Clasen's "New renderers for GTK" (28 Jan 2024): the ngl and Vulkan renderers are "unified" renderers built from one source, sharing infrastructure for "caching textures and glyphs"; render_texture to dmabuf was Vulkan-only at the time — [blog.gtk.org, Jan 2024](https://blog.gtk.org/2024/01/)
- Ghostty prior art: Ghostty's GTK frontend dropped `GtkGLArea` because "GTK's render lifecycle lives exclusively on the main UI thread", which forced a fragile hand-off with its render thread and a chicken-and-egg initialisation (wait for GTK's GL context before creating the renderer). Its replacement `RenderSurface` widget has the renderer export frames as DMABUFs into a queue that the widget consumes "whenever GTK calls for it to be snapshotted", enabling triple-buffering and being API-agnostic (GL or Vulkan producers) — [ghostty src/apprt/gtk/class/render_surface.zig, header comment](https://github.com/ghostty-org/ghostty/blob/main/src/apprt/gtk/class/render_surface.zig)
- Ghostty's `RenderSurface` builds a `GdkDmabufTexture` per present (`DmabufTextureBuilder` with fourcc/modifier/premultiplied/planes) and falls back to `GdkMemoryTexture` from a `GBytes` for CPU frames — [render_surface.zig `rebuildDmabufTexture`/`rebuildMemoryTexture`](https://github.com/ghostty-org/ghostty/blob/main/src/apprt/gtk/class/render_surface.zig)
- `GDK_DEBUG=force-offload` forces graphics offload for all textures; `GDK_DISABLE=offload` disables it; `GDK_DEBUG=offload` logs it (Wayland only) — [docs.gtk.org Running GTK apps](https://docs.gtk.org/gtk4/running.html)

### Inferences
- Tiled textures are a sound pattern, and the source explains why it pays off: with no partial uploads, upload cost scales with the size of each new `GdkTexture`. Our 128 px strips at ~1900 px wide x 4 B/px are about 1 MB per strip, against about 15.7 MB for a full 1900x2068 frame. That fits the drop from 13 to 15 ms GSK render to 3 ms. Smaller tiles (for example 64 px strips, or strips split horizontally) would shrink the upload per keystroke further, at the cost of more texture nodes.
- Setting `update_texture`/`update_region` on each rebuilt strip is still worthwhile but secondary. It lets GSK's render node diff shrink the window damage to the changed rows inside a strip instead of the whole strip, which cuts the compositor-side repaint and GSK's draw region. It will not reduce upload bytes.
- Keep a persistent `GdkMemoryTextureBuilder` per strip and only rebuild strips that actually changed. Unchanged strips must be the same `GdkTexture` object frame to frame, or the cache misses and re-uploads them.
- Do not go back to `GtkDrawingArea`/cairo for the whole editor. A cairo draw func produces a fresh cairo surface each frame, which GSK has to treat as new content, which is the 13 to 15 ms upload we measured.
- `GtkGLArea` is the wrong tool for us: we rasterise on the CPU, and Ghostty (the most prominent heavy GTK4 GL user) has walked away from it.
- If we ever move rasterisation to the GPU, the Ghostty path (render off-thread, export dmabuf, wrap in `GdkDmabufTexture` in `snapshot()`) is the GTK-sanctioned way, and dmabuf textures can be offloaded straight to a Wayland subsurface, skipping GSK compositing entirely.

### Gaps
- I found no explicit blog post or GitLab comment from Matthias Clasen or Benjamin Otte endorsing tiled `GdkMemoryTexture`s for editor content, nor any recommending against it. The endorsement above comes from reading the source, not from a maintainer statement.
- Not verified: whether GSK uploads memory textures to the GPU on a worker thread (`GDK_DISABLE=threads` exists, so some operations are threaded, but I did not trace the upload path). A Sysprof capture with `GSK_DEBUG=profile` (Vulkan) would answer it.
- Not checked: the texture-atlas size thresholds in the new 4.24 cache allocator. Small strips might land in an atlas, which would change the cost model slightly.

## GSK renderer selection: defaults by version, overriding, start-up cost, downsides of cairo

### Takeaway
On Wayland, the default since 4.16 is Vulkan, then GL, then cairo. The renderer formerly called `ngl` is now just `gl` (the old GL renderer was dropped in 4.17.4). An app can choose programmatically, either by `setenv("GSK_RENDERER", ...)` before `gtk::init()` (Ghostty does the equivalent with `GDK_DISABLE=vulkan`), or through an undocumented per-display hook. Ghostty states in source that disabling Vulkan "saves hundreds of milliseconds of initialization time on certain systems", which matches our ~23 ms Vulkan-vs-GL delta on the i7-7700HQ. Cairo starts fastest (our 147 ms median), but it is the "fallback" renderer and composites on the CPU.

### Cited Findings
- Version history from GTK NEWS — [GTK NEWS](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS):
  - 4.13.5: new `vulkan` and `ngl` unified renderers added (experimental); Vulkan enabled by default at build time.
  - 4.13.6: ngl made the default renderer; fractional scaling enabled by default with GL.
  - 4.14.0 (Mar 2024): default ngl; new renderers trigger new driver bugs, so Mesa 24.x is recommended.
  - 4.15.0: dev-cycle switch to Vulkan by default on Wayland.
  - 4.16.0 (Sep 2024): "changes the default GSK renderer to be Vulkan, on Wayland. Other platforms still use ngl ... You can still override the renderer choice using the GSK_RENDERER environment variable"; Mesa 24.2 recommended.
  - 4.16.2: "Improve startup speed by avoiding initialization of GL and Vulkan in most cases"; "Fix a crash on startup with some Vulkan drivers".
  - 4.17.0: "Renderer selection will no longer consider the GL renderer" (old one); "Allow Vulkan software rendering as fallback"; "Forbid opening displays before gtk_init()"; GL dropped GLES 2 and GL < 3.3.
  - 4.17.4: "Drop the old GL renderer".
  - 4.18.2: Windows "Don't default to Vulkan" (Windows-only regression fix).
  - 4.19.1 (shipped in 4.20): "Accept GSK_RENDERER=gl for the current gl renderer" (ngl renamed to gl).
  - 4.23.2 (shipped in 4.24): `!10046 display: Add gdk_display_get/set_prefer_vulkan()` (Benjamin Otte); 4.24.1: `!10423 gl: Create 2 fbos per frame instead of 1 per image`.
- Phoronix: GTK developers trust Vulkan on Wayland after much bug fixing but advise up-to-date drivers — [Phoronix, GTK 4.16 released](https://www.phoronix.com/news/GTK-4.16-Released)
- Source (4.24 main), renderer selection order in `gsk_renderer_new_for_surface_full`: (1) display object data `"gsk-renderer"`, (2) `GSK_RENDERER` env var, (3) backend-specific (Broadway), (4) Vulkan if the display "prefers Vulkan", (5) GL, (6) Vulkan as a fallback, (7) GL as a fallback, (8) cairo — [gsk/gskrenderer.c ~L537-760](https://github.com/GNOME/gtk/blob/main/gsk/gskrenderer.c)
- Source: GL is skipped as the primary choice if `GL_RENDERER` contains "llvmpipe". Vulkan is skipped as the primary choice if the device type is CPU, or if Vulkan reports no dmabuf formats. Vulkan's checks call `gdk_display_prepare_vulkan` and `gdk_vulkan_init_dmabuf`, which is where the initialisation time goes — [gsk/gskrenderer.c](https://github.com/GNOME/gtk/blob/main/gsk/gskrenderer.c)
- Source: only the Wayland backend calls `gdk_display_set_prefer_vulkan(display, TRUE)`, so X11 defaults to GL first — [gdk/wayland/gdkdisplay-wayland.c ~L1200](https://github.com/GNOME/gtk/blob/main/gdk/wayland/gdkdisplay-wayland.c); `gdk_display_set_prefer_vulkan` is declared in `gdkdisplayprivate.h` (private API) — [gdk/gdkdisplayprivate.h ~L216](https://github.com/GNOME/gtk/blob/main/gdk/gdkdisplayprivate.h)
- Source: the `GSK_RENDERER` lookup is cached in a function-level static, so it is read once, the first time a renderer is created — [gsk/gskrenderer.c `get_renderer_for_env_var`](https://github.com/GNOME/gtk/blob/main/gsk/gskrenderer.c)
- Source: the `"ngl"` name now triggers `g_warning("The new GL renderer has been renamed to gl...")` — [gsk/gskrenderer.c ~L498-500](https://github.com/GNOME/gtk/blob/main/gsk/gskrenderer.c)
- Documented `GSK_RENDERER` values: `help`, `broadway`, `cairo` ("the fallback Cairo renderer"), `opengl`/`gl`, `vulkan`. The page does not state the default — [docs.gtk.org Running GTK apps](https://docs.gtk.org/gtk4/running.html)
- Documented `GDK_DISABLE` values include `gl`, `gl-api`, `gles-api`, `egl`, `glx`, `vulkan`, `dmabuf`, `offload`, `threads`, `icon-nodes`. `GDK_DEBUG` behaviour values include `no-portals`, `gl-prefer-gl`, `no-vsync` ("Repaint instantly (uses 100% CPU with animations)"), `frames` (frame clock info). `GSK_DEBUG` includes `renderer`, `cache`, `diff`, `full-redraw`, `profile` (Vulkan only) — [docs.gtk.org Running GTK apps](https://docs.gtk.org/gtk4/running.html)
- `GDK_DEBUG` was split into `GDK_DEBUG` and `GDK_DISABLE` in 4.15.6 (for 4.16); `GDK_VULKAN_SKIP` was renamed `GDK_VULKAN_DISABLE` in 4.14.1; `GDK_VULKAN_DEVICE` dropped in 4.17.0 in favour of `MESA_VK_DEVICE_SELECT` — [GTK NEWS](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- Ghostty sets `GDK_DISABLE=vulkan` unconditionally before GTK initialisation (asserting `gtk.isInitialized() == 0`). The source comment says: "there can still occasionally be conflicts when Vulkan and OpenGL are used together. Disabling Vulkan also saves hundreds of milliseconds of initialization time on certain systems." It strips `GDK_DEBUG`, `GDK_DISABLE` and `GSK_RENDERER` from child process environments — [ghostty application.zig `setGtkEnv`](https://github.com/ghostty-org/ghostty/blob/main/src/apprt/gtk/class/application.zig), [ghostty surface.zig ~L1617](https://github.com/ghostty-org/ghostty/blob/main/src/apprt/gtk/class/surface.zig)
- Ghostty user logs show `GDK_DISABLE=gles-api,vulkan` at launch (gles-api was later found to block GLES-capable hardware before Ghostty's own GL 4.3 gate) — [Ghostty discussion #10426](https://github.com/ghostty-org/ghostty/discussions/10426), [Ghostty discussion #13907](https://github.com/ghostty-org/ghostty/discussions/13907)
- `GDK_DISABLE=vulkan` has been tied to a crash in at least one recent third-party report (a GNOME extension launching Ghostty segfaulting about one second after it was set) — [gnome-extension-ghostty-quick-terminal #5](https://github.com/abn/gnome-extension-ghostty-quick-terminal/issues/5) (single report, cause unclear)
- Cairo renderer is still maintained (fixes in 4.18.2, 4.19.1 "Improve cairo blur performance with fractional scales", 4.21.5 "Memory leak with cairo renderer + wayland GDK backend") — [GTK NEWS](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)

### Inferences
- Recommended default for foo_edit: set `GSK_RENDERER=gl` (or `GDK_DISABLE=vulkan`, which keeps the GL then cairo fallback chain intact) via `std::env::set_var` before `gtk::init()`, unless the user already set it. This copies Ghostty's production choice, buys back ~23 ms of renderer initialisation on our hardware (and more on slower Vulkan stacks), and costs us nothing, because our content is CPU-rasterised memory textures, which both GPU renderers upload in much the same way. In Rust 2024, `set_var` is `unsafe`, so call it first thing in `main` before any threads exist. Then strip the variable from any child process we spawn (as Ghostty does).
- Prefer `GDK_DISABLE=vulkan` over `GSK_RENDERER=gl`. An explicit `GSK_RENDERER` failure is "warn_if_fail" and then falls through, but `GDK_DISABLE=vulkan` also stops other GTK internals (dmabuf import probing via Vulkan) from initialising Vulkan.
- The `"gsk-renderer"` display object data hook (`g_object_set_data(display, "gsk-renderer", "gl")`, reachable from Rust via `ObjectExt::set_data`) is a true programmatic override with priority over the env var. It is undocumented and private, so treat it as unsupported: use it only behind a fallback, if at all.
- Forcing cairo is not recommended as the default. It is the documented "fallback", every texture node is composited on the CPU (at 4K x2 that means compositing ~16 MB per full repaint), and GPU-only features (offload to subsurfaces, cheap transforms and blurs for any libadwaita chrome) go away. Our cold start gain (~66 ms median against GL) is real, so it could be an opt-in "low-latency start" setting, but we have not yet measured cairo's keystroke p95 against GL. Measure before deciding.
- `gsk_renderer_new_for_surface()` (public) lets you create a renderer for your own surface, but a `GtkWindow` creates and owns its renderer internally, so this does not help a normal window pick a renderer.

### Gaps
- No primary-source numbers for Vulkan vs GL initialisation cost beyond Ghostty's "hundreds of milliseconds on certain systems". Our ~100 ms vs ~77 ms appear to be the best figures available for this hardware.
- I did not find a GitLab issue titled as a start-up regression in 4.20 to 4.24. The 4.16.2 "avoid initialization of GL and Vulkan in most cases" fix is the only explicit start-up entry in NEWS since 4.14.
- Not verified: whether 4.24's `prefer_vulkan` will become public API (it is private as of 4.24.1).

## GTK start-up time: libraries, gtk_init, GApplication, icon theme and CSS, what fast GTK apps do

### Takeaway
GTK upstream offers little start-up-specific guidance. The practical lessons come from Ghostty: most of the cost is GTK overhead, and the real fix is to stay resident (single instance plus D-Bus/systemd activation) so a "new window" costs ~33 ms instead of a 200 to 500 ms cold start. For a one-shot editor launch, the levers we control are renderer choice, fewer linked libraries (prelinking or static linking where we can), `G_APPLICATION_NON_UNIQUE` or a deliberate single-instance design, and measuring "request to window accepting input" rather than process exit.

### Cited Findings
- Ghostty discussion "How to get fast startup on Linux?" (Debian 13.3, Ghostty 1.3.0-dev ReleaseFast): first launch ~500 ms; second launch with `--gtk-single-instance=true` ~200 ms (measured 206 ms); `ghostty +new-window` 33 ms; `xterm -e true` 45 ms — [Ghostty discussion #10426](https://github.com/ghostty-org/ghostty/discussions/10426)
- Maintainer (jcollie), marked as the answer: Ghostty will likely never start as fast as xterm because GTK trades some start-up speed for functionality; use systemd and D-Bus activation; measure from the user's request until the window accepts input, not launcher exit; ~200 ms is near human reaction time, so further gains are pursued only if easy — [Ghostty discussion #10426](https://github.com/ghostty-org/ghostty/discussions/10426)
- Collaborator (hqnna): most start-up time is GTK overhead, not Ghostty. Recommends the systemd service plus `gtk-single-instance = true`, `quit-after-last-window-closed = true`, `quit-after-last-window-closed-delay = 5m` — [Ghostty discussion #10426](https://github.com/ghostty-org/ghostty/discussions/10426)
- GTK 4.16.2: "Improve startup speed by avoiding initialization of GL and Vulkan in most cases" — [GTK NEWS, 4.16.2](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- GTK 4.17.0: "Forbid opening displays before gtk_init()"; GtkApplication "Respect GDK_DEBUG=no-portals" and "Set default window icon from app ID" — [GTK NEWS, 4.17.0](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- GTK 4.21.0: "The Wayland backend relies on the portal for settings" — [GTK NEWS, 4.21.0](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS); `GDK_DEBUG=no-portals` "Disable use of portals" — [docs.gtk.org Running GTK apps](https://docs.gtk.org/gtk4/running.html)
- GTK 4.23.0 (4.24): "GtkSvg is now used for all SVG icons"; `GDK_DISABLE=icon-nodes` disables svg-to-node conversion for symbolic icons — [GTK NEWS, 4.23.0](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS), [docs.gtk.org Running GTK apps](https://docs.gtk.org/gtk4/running.html)
- Older (2022, GTK 4.6 era, flag as pre-2023): GTK issue #5263 reports libadwaita apps taking 2 to 3 s to launch on Fedora 36 versus fast GTK3 apps, blamed on custom icon packs and `g_main_context_iterate` — [GNOME/gtk#5263](https://gitlab.gnome.org/GNOME/gtk/-/issues/5263)
- Sysprof can record GTK frame-clock timings — [AlternativeTo Sysprof listing](https://alternativeto.net/software/sysprof/about); Christian Hergert's Sysprof posts (2023: "How to use Sysprof (again)", frame pointers) are listed on the GNOME wiki — [wiki.gnome.org Sysprof](https://wiki.gnome.org/Apps/Sysprof)

### Inferences
- The portal settings lookup (4.21+ on Wayland) is a D-Bus round trip at start-up. Testing `GDK_DEBUG=no-portals` would show its cost, but do not ship that (it breaks dark mode and accent colour sync).
- `G_APPLICATION_NON_UNIQUE` skips the D-Bus name ownership round trip. The trade-off is losing "open file in existing window". For an editor, a Ghostty-style resident instance (D-Bus activatable, `gapplication` remote invocation forwarding files) gives far better repeat-launch latency than any cold-start tuning. That is the design decision to make, not a flag to flip.
- Our measured 35 to 55 ms of dynamic loading for ~113 libraries is likely the next biggest fixed cost after renderer initialisation. Options: avoid libadwaita if we do not need it (it adds its own library and stylesheet), check which libraries GTK pulls in purely for unused features (for example the media backend or print backends, which load lazily as modules and should not be on the critical path), and make sure we link with `-Wl,--as-needed`.
- Measure "keyboard focus ready" (first frame presented plus input surface mapped) using `GdkFrameTimings` presentation time on the first frame, which our spike already has.

### Gaps
- No primary-source profiling numbers found for `gtk_init` cost, CSS/theme load, or icon theme load in GTK 4.20+. A Sysprof capture of our binary is the way to get them.
- I found no published start-up write-ups for gnome-text-editor or Ptyxis. Both are GNOME apps using GApplication single-instance and libadwaita; I could not confirm any start-up-specific technique in their repositories within the time budget.
- No source quantified libadwaita's start-up cost in 4.20+.

## Frame clock and input latency on Wayland

### Takeaway
GTK's frame clock on Wayland is throttled by the compositor's `wl_surface.frame` callback. After input, the widget's `queue_draw()` requests a paint, but the paint cannot start while the clock is "throttled" waiting for the previous frame's callback. A keystroke landing just after a commit can therefore wait up to roughly two refresh intervals (about 33 ms at 60 Hz), which matches our 36 ms worst-case wait. There is no public "paint now" API. `GDK_DEBUG=no-vsync` is a debug flag only. GTK 4.23/4.24 reworked the frame clock specifically "to improve precision and reduce frame drops", so 4.24 is the baseline to measure against.

### Cited Findings
- Source: `should_run_source()` only schedules a paint when the clock is not stopped, not in a frame, `gdk_frame_clock_get_throttling(clock) == 0`, and a phase is requested. The paint source is a timeout of `min_next_frame_time - now` (zero if already past) at `GDK_PRIORITY_REDRAW`, or `GDK_PRIORITY_EVENTS + 1` if `FLUSH_EVENTS` is requested. `GDK_DEBUG=no-vsync` bypasses the minimum-interval wait — [gdk/gdkframeclockidle.c ~L155-215](https://github.com/GNOME/gtk/blob/main/gdk/gdkframeclockidle.c)
- Source: the Wayland surface requests `wl_surface_frame()` on commit and, in the frame-done callback, calls `gdk_draw_context_frame_stop_throttling()`, so throttling is released by the compositor's frame callback — [gdk/wayland/gdksurface-wayland.c ~L298-380, L516-518](https://github.com/GNOME/gtk/blob/main/gdk/wayland/gdksurface-wayland.c)
- GTK 4.14.2: Wayland "Use the presentation time protocol" — [GTK NEWS, 4.14.2](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- GTK 4.23.2: "The first part of ongoing frameclock work has landed, improving our ability to deliver frames on time" (!10005, !10029, !10035; Benjamin Otte, Christian Hergert) — [GTK NEWS, 4.23.2](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- GTK 4.23.3: "The GDK frameclock has been reworked to improve precision and reduce frame drops" (!10108, !10122 deadlock fix, !10141 "Redo predicted presentation time") — [GTK NEWS, 4.23.3](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- GTK 4.23.4: "!9982 inspector: Add a frametime overlay" (Benjamin Otte); 4.24.0: "!10361 Some frame clock fixes" (Matthias Clasen) — [GTK NEWS](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- GTK 4.17.4: Vulkan "Handle VK_PRESENT_MODE_FIFO_LATEST_READY_EXT" — [GTK NEWS, 4.17.4](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- GTK 4.17.0: GtkTextView positioning quantised to device pixels for smoother scrolling under fractional scaling; 4.23.1: "Snapping: GTK can now snap contents to device pixel boundaries, for better control of rendering with fractional scales" — [GTK NEWS](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- `GDK_DEBUG=frames` gives frame clock information; `no-vsync` "Repaint instantly (uses 100% CPU with animations)" — [docs.gtk.org Running GTK apps](https://docs.gtk.org/gtk4/running.html)
- Ghostty's `RenderSurface` pulls the newest frame from a queue at snapshot time (triple-buffered), so render work never blocks the frame clock — [render_surface.zig](https://github.com/ghostty-org/ghostty/blob/main/src/apprt/gtk/class/render_surface.zig)

### Inferences
- For our p95 of 14 ms and p99 of 48 ms, the controllable parts are: (a) keep `snapshot()` and texture building well under one frame (already 0.7 ms draw plus 3 ms GSK); (b) avoid ever missing the frame callback window, since a missed frame doubles latency; (c) do the text-model update directly in the key handler and call `queue_draw()` immediately, never from an idle or timeout.
- The 48 ms p99 tail likely comes from the frame clock waiting through one full throttled cycle, plus occasional main-loop stalls. To tell them apart, log `GdkFrameClock::layout`/`paint` timestamps and `GdkFrameTimings` presentation times against the key event timestamp, and check for long main-loop iterations.
- Hyprland's own frame-callback timing (when it sends `frame done` relative to its vblank and render) sets the floor. Compositor options such as Hyprland's render-ahead or direct scan-out for fullscreen windows are outside GTK's control and worth testing separately.
- `GDK_DEBUG=no-vsync` is useful in the lab to measure "how much latency is frame-clock wait", but should not ship.

### Gaps
- Not found: GTK maintainer statements or public API for "paint immediately on input". Nothing in 4.24 NEWS adds one.
- Not found within budget: VTE/gnome-terminal latency improvement write-ups from 2023 onward, or Ghostty input-latency measurements on GTK (the latency figures in a LinuxJunkies article look like unsourced marketing and were excluded).
- Not verified: whether the reworked 4.24 frame clock changes the throttling logic I read (the source read is from main after 4.24.1, so it should reflect 4.24).

## Ghostty's GTK4 frontend: start-up, renderer, latency, X11, fractional scaling, libadwaita

### Takeaway
Ghostty's GTK frontend is the closest prior art, and its current choices are: no `GtkGLArea`; render off the main thread and hand frames to a custom widget as `GdkDmabufTexture` (memory-texture fallback); force `GDK_DISABLE=vulkan` for start-up time and GL/Vulkan coexistence; strip GTK debug env vars from children; and rely on single-instance plus systemd/D-Bus activation for fast new windows.

### Cited Findings
- `GtkGLArea` was abandoned because GTK's render lifecycle lives exclusively on the main thread, initialisation had to wait for GTK's GL context, and a dedicated render thread had to coordinate with GTK's draw — [render_surface.zig header](https://github.com/ghostty-org/ghostty/blob/main/src/apprt/gtk/class/render_surface.zig)
- `GDK_DISABLE=vulkan` is set unconditionally before GTK init, citing both GL/Vulkan conflicts and "hundreds of milliseconds" of init time — [application.zig `setGtkEnv`](https://github.com/ghostty-org/ghostty/blob/main/src/apprt/gtk/class/application.zig)
- Older Ghostty commits (Nov 2024) also prevented Vulkan initialisation for start-up time and forced `GSK_RENDERER=ngl` on GTK 4.14+. They handled the 4.16 `GDK_DEBUG`/`GDK_DISABLE` split by version — [Ghostty commit history mirror](https://git.uoc.run.place/Applied-Software/ghostty/commits/commit/27c3382a6a6b1d87262fe2f21d68242093c6950d/src/apprt/gtk/App.zig) (mirror, not the canonical repo)
- Ghostty's Linux renderer needs desktop OpenGL 4.3; crashes on GL-limited hardware appear in user reports — [Ghostty discussion #13907](https://github.com/ghostty-org/ghostty/discussions/13907), [ghostty-ubuntu #156](https://github.com/mkasberg/ghostty-ubuntu/issues/156)
- Ghostty's EGL display comes from either the Wayland or the X11 GDK display, so both backends are supported — [surface.zig ~L1792-1798](https://github.com/ghostty-org/ghostty/blob/main/src/apprt/gtk/class/surface.zig)
- Ghostty logs per-surface scale changes (CSS size vs device size, surface scale vs scale factor) in `RenderSurface`, showing that fractional scale has to be tracked explicitly from `GdkSurface:scale` rather than the integer `scale-factor` — [render_surface.zig scale logging](https://github.com/ghostty-org/ghostty/blob/main/src/apprt/gtk/class/render_surface.zig)
- Start-up numbers and the maintainers' single-instance advice: see the start-up section above — [Ghostty discussion #10426](https://github.com/ghostty-org/ghostty/discussions/10426)

### Inferences
- For fractional scaling (Hyprland at 1.25/1.5 and so on), rasterise at `surface.scale()` (f64, `GdkSurface:scale`, 4.12+) rather than `widget.scale_factor()` (an integer, rounded up), and size textures in device pixels so GSK draws them 1:1 with no resampling. The 4.23.1 snapping feature helps keep texture nodes on device pixel boundaries.
- libadwaita: Ghostty uses it (its logs show libadwaita version info), but no source quantifies its start-up cost. For a plain editor, skipping libadwaita removes a library and a stylesheet from start-up. Weigh that against GNOME platform consistency.

### Gaps
- Not found: specific Ghostty issues documenting fractional-scaling bugs or X11-specific problems in the 1.3/1.4 era. I did not have the budget to search their issue tracker exhaustively.
- Not measured anywhere I found: libadwaita's start-up overhead.

## X11 support status in GTK 4.x

### Takeaway
The X11 backend was deprecated in GTK 4.17.4 (Feb 2025, so from 4.18), alongside Broadway. It is slated for removal in GTK 5 but stays in GTK 4 for GTK 4's whole lifetime. For a Wayland-primary, X11 best-effort app on GTK 4, X11 keeps working. Expect no new X11 features, and note that on X11 the renderer defaults to GL, not Vulkan.

### Cited Findings
- GTK 4.17.4 NEWS: "Deprecations: The x11 backend, The Broadway backend" (!8060 "Deprecate the X11 backend") — [GTK NEWS, 4.17.4](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- Phoronix: GTK 4.17.4 deprecates the X11 and Broadway backends, which are planned for removal in GTK 5 — [Phoronix](https://www.phoronix.com/news/GTK-X11-Now-Deprecated)
- OSNews quoting the GTK project: X11 support will still exist until GTK 4 is EOL, which happens once GTK 6 is released — [OSNews](https://www.osnews.com/story/141670/gtk-announces-x11-deprecation-new-android-backend-and-much-more/)
- GNOME Discourse: whether `GDK_BACKEND` stays was undecided — [Discourse: Deprecated X11 backend](https://discourse.gnome.org/t/deprecated-x11-backend/30510)
- GTK 4.19.2: "Don't build the x11 backend on macos, by default"; Broadway renderer deprecated in 4.19.3 — [GTK NEWS](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- X11 bug fixes continue to land in 4.23/4.24 (for example #6493 X11 selection CPU usage, #3542/#5610 touch window dragging, 4.24 `gdkclipboard-x11` NULL converter fix) — [GTK NEWS](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)
- Only the Wayland GDK display sets prefer-Vulkan, so X11 picks GL first — [gdk/wayland/gdkdisplay-wayland.c](https://github.com/GNOME/gtk/blob/main/gdk/wayland/gdkdisplay-wayland.c), [gsk/gskrenderer.c](https://github.com/GNOME/gtk/blob/main/gsk/gskrenderer.c)
- 4.19.2: GTK relies on the Wayland text-input protocol for input methods; set `GTK_IM_MODULE=simple` on compositors without it to get X11-style compose and dead keys — [GTK NEWS, 4.19.2](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)

### Inferences
- "X11 best-effort" matches upstream's stance well. Do not depend on X11-only APIs (`gdk4-x11` crate) except behind a feature gate, since GTK 5 will not have them.
- The input-method change in 4.19.2 matters for an editor: test compose and dead keys on Hyprland, which supports text-input-v3, and on any compositor that lacks it.

### Gaps
- No GTK 5 release date or concrete X11 removal version found as of Nov 2026.

## Distro GTK4 versions (relevant to requiring 4.16+ APIs)

### Takeaway
Requiring GTK 4.16 (for `GdkMemoryTextureBuilder` update regions) excludes Ubuntu 24.04 LTS (4.14), Debian 12 (4.8) and openSUSE Leap 15.6 (4.12). Every current non-LTS or newer release has 4.18 or later: Debian 13 (4.18), Fedora 42/43/44 (4.18/4.20/4.22), Ubuntu 26.04 LTS (4.22), Arch and Tumbleweed (4.24).

### Cited Findings
All from Repology's `gtk` project, queried 2026-11-03 — [Repology gtk](https://repology.org/project/gtk/versions):
- Ubuntu 22.04: 4.6.9; Ubuntu 24.04 LTS: 4.14.5 (release 4.14.2); Ubuntu 25.04: 4.18.5; Ubuntu 25.10: 4.20.1; Ubuntu 26.04 LTS: 4.22.4 (release 4.22.2); Ubuntu 26.10: 4.24.1
- Debian 12 (bookworm): 4.8.3; Debian 13 (trixie): 4.18.6; Debian 14/unstable: 4.24.1
- Fedora 41: 4.16.13; Fedora 42: 4.18.6; Fedora 43: 4.20.4; Fedora 44: 4.22.5; Rawhide: 4.24.1
- openSUSE Leap 15.6: 4.12.4; Tumbleweed: 4.24.1
- Arch: 4.24.1
- Ubuntu noble source page confirms gtk4 4.14.2+ds-1ubuntu1 — [packages.ubuntu.com noble gtk4](https://packages.ubuntu.com/source/noble/gtk4)
- GTK release dates (from NEWS): 4.14.0 12-03-2024, 4.16.0 09-06-2024 (Sep 2024), 4.18 ~Mar 2025, 4.20.0 29-08-2025, 4.22.0 06-03-2026, 4.24.0 11-09-2026 — [GTK NEWS](https://raw.githubusercontent.com/GNOME/gtk/main/NEWS)

### Inferences
- A 4.16 floor is reasonable if Ubuntu 24.04 users can get us via Flatpak (GNOME runtime) instead of distro packages. Otherwise make the update-region path optional with gtk4-rs feature `v4_16` and runtime `gtk::check_version`. The plain `GdkMemoryTexture::new` path works back to 4.0. Our tiling gives most of the win without update regions.
- gtk4-rs 0.11 feature flags let us compile against a 4.14 floor and enable 4.16 APIs conditionally.

### Gaps
- openSUSE Leap 16.0 did not appear in the Repology output, so its GTK4 version is unverified.
- Repology "legacy" vs "outdated" entries reflect release vs updates pockets. Exact point versions in stable updates may change.
