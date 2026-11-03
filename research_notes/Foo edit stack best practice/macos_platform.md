# macOS platform best practice: frame presentation and app-bundle process behaviour

Scope: a CPU-rasterised (tiny-skia) Rust text editor on AppKit via objc2. Notes check our spike findings (CGImage contents ~12 ms per commit; IOSurface double buffer 0.12 ms commit, keystroke p95 4.4 ms; terminal-spawned window not key in a background bench) against Apple docs, SDK headers and prior art.

Source note: several Apple documentation pages render client-side and could not be fetched as text. Where that happened I read the macOS SDK headers on the research machine instead (Xcode SDK reporting version 27.2, path `/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk`). Header citations are given as `SDK: <header>:<line>`; they are primary Apple sources but are not URLs. Fetch budget was limited (about 15 calls), so some questions are only partly answered and are listed under Gaps.

## 1. Why CALayer.contents = CGImage costs ~12 ms per commit, and CGImage vs IOSurface

### Takeaway
Our 12 ms finding is corroborated by independent measurements in the softbuffer crate (18 to 20 ms per present with a fresh CGImage per frame, versus 0.74 ms when presenting IOSurfaces). The prior-art consensus is that a CGImage set as layer contents is copied (and uploaded) by Core Animation on each commit, whereas an IOSurface is read in place by the compositor. IOSurface contents with front/back swapping is the right CPU-rendered path; `contentsFormat` is irrelevant to it.

### Cited Findings
- Apple's CALayer header says `contents` is "an object providing the contents of the layer, typically a CGImageRef or an IOSurfaceRef, but may be something else". IOSurface is therefore a documented, first-class contents type, not a hack. (SDK: QuartzCore/CALayer.h:373-378)
- `contentsFormat` is "a hint for the desired storage format of the layer contents provided by -drawLayerInContext. Defaults to kCAContentsFormatRGBA8Uint. Note that this does not affect the interpretation of the `contents' property directly." So setting `contentsFormat` does nothing for a layer whose contents we assign ourselves. (SDK: QuartzCore/CALayer.h:430-435; available values RGBA8Uint, RGBA16Float, Gray8Uint, Automatic at lines 1032-1041)
- Apple's `CALayer.contents` documentation warns that for a layer that backs a view, you should avoid setting `contents` directly because the view may overwrite it during a later update ([Apple: CALayer.contents](https://developer.apple.com/documentation/quartzcore/calayer/contents.md), via search summary). This argues for a layer-hosting view, or a dedicated sublayer we own, rather than poking the backing layer of a layer-backed view.
- Apple docs do not state that a CGImage is copied at commit, nor do they mention a zero-copy guarantee for IOSurface. I found no official Apple statement either way (search summary of [Apple: CALayer.contents](https://developer.apple.com/documentation/quartzcore/calayer/contents.md)).
- softbuffer (the main Rust CPU-presentation crate) currently creates a new CGImage over a `CGDataProvider` on every `present()`, wraps `setContents` in a `CATransaction` with actions disabled (to suppress the default fade), creates its colour space once, never sets `contentsFormat`, and allocates a fresh zeroed buffer for every frame in `buffer_mut()` ([softbuffer 0.4.8 cg.rs](https://docs.rs/crate/softbuffer/latest/source/src/backends/cg.rs)). This is essentially the design our spike started with.
- softbuffer issue #83 (opened 31 Mar 2023 by maintainer ids1024) proposes IOSurface front/back buffers written via `IOSurfaceGetBaseAddress`. ids1024 notes the CGImage route "still requires CoreGraphics to upload the data" even with a custom data provider. IOSurface needs a stride API because row stride cannot always equal width ([softbuffer #83](https://github.com/rust-windowing/softbuffer/issues/83)).
- Measurements in #83 (LoganDark, 25 Aug 2023, Intel Mac, `Instant::now()` timing): largest present time `master` (CGImage) 19.8 ms, `copy-to-iosurface` 13.0 ms, `iosurface-wip` (render directly into IOSurface) 0.74 ms. Direct IOSurface was the most consistent; copying into an IOSurface on present was dropped as not worth it ([softbuffer #83](https://github.com/rust-windowing/softbuffer/issues/83)).
- Also in #83: lmglmg (Feb 2024) reports CGImage `master` uses very large amounts of memory during continuous resize on M1, fixed by the IOSurface branch; nicoburns (Jul 2025) reports 1 to 2 ms present with the `pixels` crate (wgpu) versus 18 to 20 ms with softbuffer; madsmtm (Aug 2025) recommends front/back IOSurface buffering with present swapping the layer contents, says CVPixelBuffer is "a red herring" because it just abstracts over IOSurface or memory, and points to PR #275 which disables costly CATransaction actions. Issue #279 ("slow present()") was closed as a duplicate, and #83 was put on the v0.5 milestone in Jan 2026 ([softbuffer #83](https://github.com/rust-windowing/softbuffer/issues/83)). As of softbuffer 0.4.8 the IOSurface path is not shipped.
- The `denise-macos` crate (third-party, not Apple) documents the same design: two IOSurfaces "shown alternately", "handed to a CALayer as its contents, where CoreAnimation reads it in place, no copy", versus a CGImage per frame which is "copied whole on every commit however little of it changed". Its example: 1040x720 surface with one spinner animating costs 9.2% of a core with CGImage versus 2% with IOSurface. It notes IOSurfaceRef is toll-free bridged to the IOSurface class, "which is what makes it assignable to contents at all" ([denise_macos ViewSurface](https://docs.rs/denise-macos/latest/denise_macos/struct.ViewSurface.html)).
- Same source: assigning the same IOSurface object to `contents` again "tells CoreAnimation nothing", so a single surface leaves "the window show[ing] the first frame for ever". Swapping between two surfaces forces a change every present. The private `-[CALayer setContentsChanged]` is the alternative, "and not one a published crate should take" ([denise_macos ViewSurface](https://docs.rs/denise-macos/latest/denise_macos/struct.ViewSurface.html)).
- A developer forums thread found a CVPixelBuffer assigned to `contents` only worked when it was IOSurface-backed with an RGB format, with varying behaviour across devices and no Apple answer ([Apple Developer Forums 673868](https://developer.apple.com/forums/thread/673868)).
- On colour: once a layer's contents carry a colour space, Core Animation "performs any needed transformations when compositing" (stated on the CAMetalLayer colorspace page) ([Apple: CAMetalLayer.colorspace](https://developer.apple.com/documentation/quartzcore/cametallayer/colorspace.md)). Retagging a CGImage with `copy(colorSpace:)` only replaces the tag and does no colour matching ([Apple: CGImage.copy(colorSpace:)](https://developer.apple.com/documentation/coregraphics/cgimage/copy(colorspace:).md); [quartz-dev thread](https://groups.google.com/g/quartz-dev/c/CamwZj3YQk8)).

### Inferences
- Our ~12 ms is in the same band as softbuffer's 13 to 20 ms, on different hardware, so it is very likely intrinsic to the CGImage-as-contents path, not a bug in our spike.
- The cost being independent of image size (128 px strip versus whole window) suggests the dominant cost is a fixed per-commit cost (for example a synchronous handoff of a new image object to the render server, or creating and uploading a new backing texture) rather than per-byte copy bandwidth. That is a hypothesis: no source confirms the exact mechanism. Display P3 being twice as slow is consistent with an extra colour-matching pass when the image's colour space differs from what the compositor wants, but again this is inference.
- `contentsFormat` and `drawsAsynchronously` are not levers for us: the former only affects `drawLayerInContext` backing stores (header), and the latter is about CG drawing into the layer.
- Recommendation: keep the IOSurface path; drop CGImage entirely; use a layer we own (layer-hosting view, or a sublayer) so AppKit cannot overwrite `contents`; wrap the swap in a CATransaction with actions disabled (as softbuffer does) or set the layer's `actions` to null for `contents`.

### Gaps
- No Apple engineer statement found on what exactly happens to a CGImage at commit (copy in-process via CA::Render, decode, colour match, or upload in WindowServer). Confirming would need an Instruments trace (Time Profiler plus System Trace across our process and WindowServer) of a CGImage commit.
- No WWDC transcript was retrieved on IOSurface layer contents in this pass.

## 2. What comparable projects do to present frames on macOS

### Takeaway
No serious low-latency macOS app presents per-frame CGImages. GPU-rendered apps use Metal, and the recent trend (Ghostty 2025) is to skip CAMetalLayer and set IOSurfaces directly as CALayer contents; Chromium has long shipped IOSurfaces into CALayer trees across processes. For a CPU renderer, IOSurface contents is the matching and recommended path.

### Cited Findings
- Ghostty's "Big Renderer Rework" (PR #7620, opened 17 Jun 2025, merged 22 Jun 2025) replaced CAMetalLayer with a custom `IOSurfaceLayer` in a layer-hosting view. The PR says the rework "slightly reduced the memory footprint", attributed to "removing the overhead of CAMetalLayer". It also draws synchronously on the main thread during resize so "a frame of the correct size" is always available, and draws the background in GPU code instead of setting a layer background colour to avoid holes during resize ([Ghostty PR #7620](https://github.com/ghostty-org/ghostty/pull/7620); older `CAMetalLayer` field visible in a [Metal.zig mirror](https://git.uoc.run.place/Applied-Software/ghostty/src/commit/00cce91dc4d8382aa10fbc2cba83d1cff82e6384/src/renderer/Metal.zig)).
- Ghostty defaults its colour handling to "native" on macOS "to match other mac apps in appearance", because macOS users are sensitive to text rendering ([Ghostty PR #7620](https://github.com/ghostty-org/ghostty/pull/7620)).
- Chromium's `ui/accelerated_widget_mac/ca_layer_tree_coordinator` owns the CALayer tree for composited content; it is created in the GPU process and shared to the browser process through the cross-process Core Animation mechanism (a CAContext whose ID is sent across, the "remote layer" path). It can also expose the frame's backbuffer as an IOSurface when remote layers are not used, and later versions add a queue of presented frames and a fullscreen low-power AVSampleBufferDisplayLayer path ([Chromium ca_layer_tree_coordinator.h](https://chromium.googlesource.com/chromium/src/+/b42c975183b2103c5c039d850926386f631ffe87/ui/accelerated_widget_mac/ca_layer_tree_coordinator.h); [.mm](https://chromium.googlesource.com/chromium/src/+/af7f13600046b6fa4c007577e7fe6b24e1038903/ui/accelerated_widget_mac/ca_layer_tree_coordinator.mm)).
- Flutter's iOS software rendering surface and Firefox/Thunderbird's `QuartzSupport.mm` both appear in searches for IOSurface layer contents, showing the IOSurface-as-contents pattern is used by major engines for software or GL fallbacks ([Flutter ios_surface_software.mm](https://flutter.googlesource.com/mirrors/engine/+/06ca07cbcfe083a1ec41e490c8cda8e49b24b4c2/shell/platform/darwin/ios/ios_surface_software.mm); [Mozilla QuartzSupport.mm](https://grok.dragonflybsd.org/xref/dports/mail/thunderbird/thunderbird-91.8.0/gfx/2d/QuartzSupport.mm)). I did not read these files in detail.
- softbuffer (used by many winit apps for CPU drawing) still uses CGImage per frame as of 0.4.8 and has an open, milestoned issue to move to IOSurface ([softbuffer cg.rs](https://docs.rs/crate/softbuffer/latest/source/src/backends/cg.rs); [softbuffer #83](https://github.com/rust-windowing/softbuffer/issues/83)).
- The wgpu-based `pixels` crate presents in 1 to 2 ms on macOS versus 18 to 20 ms for softbuffer, per a 2025 user report ([softbuffer #83](https://github.com/rust-windowing/softbuffer/issues/83)).

### Inferences
- Using softbuffer as-is on macOS would reintroduce our 12 ms regression. If we want a crate, it would have to be the IOSurface branch or our own backend (which the spike already has).
- Uploading tiny-skia output into a Metal texture and presenting via CAMetalLayer is an alternative, but it adds a GPU copy and drawable-acquisition waits; Ghostty moving off CAMetalLayer suggests IOSurface contents is at least as good for latency and memory.

### Gaps
- Zed (GPUI), Alacritty, WezTerm, kitty and Neovide presentation code was not read in this pass. From general knowledge they are GPU renderers (Zed and recent WezTerm via Metal, Alacritty and kitty via OpenGL, Neovide via Skia on GL/Metal), but this is unsourced here and should be checked before quoting.
- No source found on "CARemoteLayer" by that name beyond Chromium's remote-layer (CAContext) usage.

## 3. Double-buffering, tearing, IOSurface locks and colour space tagging

### Takeaway
Two IOSurfaces swapped as `contents` is the documented-in-practice pattern: it both signals a change to Core Animation and avoids writing into the surface the compositor is reading. Bracket CPU writes with `IOSurfaceLock`/`IOSurfaceUnlock`, and tag each surface with `kIOSurfaceColorSpace` (serialised CGColorSpace) so colours are correct on P3/XDR displays.

### Cited Findings
- Two surfaces alternate so every present assigns a different object; the buffer you get back is "two frames old, not one", so damage tracking must cover two frames of damage (buffer age 2) ([denise_macos ViewSurface](https://docs.rs/denise-macos/latest/denise_macos/struct.ViewSurface.html)).
- softbuffer maintainers raised the open question of how to know when a buffer is safe to reuse (for the CGImage route the data provider's release callback signals it) and wondered about CPU write performance to IOSurfaces on discrete GPUs ([softbuffer #83](https://github.com/rust-windowing/softbuffer/issues/83)).
- IOSurface lock options are `kIOSurfaceLockReadOnly` (0x1) and `kIOSurfaceLockAvoidSync` (0x2) (SDK: IOSurface/IOSurfaceTypes.h:21-26). The lock function's doc refers to these enums (SDK: IOSurface/IOSurfaceRef.h:200).
- `kIOSurfaceColorSpace` "defines the RGB (or other) color space for the data stored in the IOSurface. It is expected that this is a serialized CGColorSpaceRef, provided by the CoreGraphics CGColorSpaceRef serialization APIs (CGColorSpaceCopyPropertyList)". There is also an ICC profile key taking CFData from `CGColorSpaceCopyICCData`. Available since macOS 10.6 (SDK: IOSurface/IOSurfaceRef.h:302-307).
- The CALayer header mentions IOSurfaces carrying `kIOSurfaceContentHeadroom` for EDR, in which case the layer's EDR property "does not need to be set" (SDK: QuartzCore/CALayer.h:475). EDR-capable contents can cost "significant" power and should only be enabled for EDR content (SDK: QuartzCore/CALayer.h:437-446).

### Inferences
- Tearing: with two surfaces, writing into the back surface while the front is displayed cannot tear the visible frame, provided we never write into the surface currently set as `contents`. If the compositor still holds the previous front surface when we start writing (we swap faster than display refresh), we could in theory write into a surface being scanned out; pacing presents to the display link (section 4) avoids this. `IOSurfaceIsInUse` exists for checking this, but I did not source its semantics in this pass.
- Colour: tag surfaces sRGB for an editor whose theme colours are authored in sRGB, and let the compositor match to the P3/XDR panel. Our spike's finding that Display P3 tagging was slower (with CGImage) is a reason to author in sRGB unless the theme needs wide gamut. Untagged surfaces risk being treated as device RGB and looking oversaturated on P3 panels (inference, not sourced).
- Writing only damaged rows into a two-frame-old buffer requires accumulating damage across both frames (or copying the delta from the front buffer); our spike's "damaged rows only" must account for this or it will show stale rows.

### Gaps
- No Apple doc retrieved confirming whether `IOSurfaceLock` is required for CPU writes to a surface the window server may read, or the cost of lock/unlock on Apple Silicon.
- No source on whether Core Animation honours `kIOSurfaceColorSpace` for layer contents on all macOS versions (it is widely assumed; verify visually on the XDR panel with a known P3 test colour).

## 4. Display link and frame pacing on macOS 14+, and measuring on-glass latency

### Takeaway
CVDisplayLink is formally deprecated in the macOS 15 SDK; the replacement is `NSView`/`NSWindow`/`NSScreen` `displayLink(target:selector:)`, which returns a CADisplayLink tied to the view's current display (macOS 14+). Use the view variant so pacing follows the window between the XDR panel and the 5K external.

### Cited Findings
- CVDisplayLink APIs are wrapped in `API_DEPRECATED_BEGIN("use NSView.displayLink(target:selector:), NSWindow.displayLink(target:selector:), or NSScreen.displayLink(target:selector:)", macos(10.4, 15.0))` (SDK: CoreVideo/CVDisplayLink.h:51). QEMU's build output confirms "first deprecated in macOS 15.0" ([QEMU issue 2575](https://gitlab.com/qemu-project/qemu/-/issues/2575); [QEMU patch](https://patchew.org/QEMU/20241106162150.522782-1-pbonzini@redhat.com)). An OpenJDK bug says deprecated "from macOS 14" ([JDK-8357418](https://bugs.openjdk.org/browse/JDK-8357418)); the SDK header settles it as 15.0.
- `-[NSView displayLinkWithTarget:selector:]` "Returns a new display link whose callback will be invoked in-sync with the display the view is on. If the view is hidden, or not on any display, the callback will not be invoked." Available macOS 14.0 (SDK: AppKit/NSView.h:656-662; [Apple: NSView.displayLink(target:selector:)](https://developer.apple.com/documentation/appkit/nsview/displaylink(target:selector:).md)).
- OpenJDK notes CADisplayLink / CAMetalDisplayLink handle display sleep and wake, removing custom handling ([JDK-8357418](https://bugs.openjdk.org/browse/JDK-8357418)).

### Inferences
- For an editor, do not render continuously on the display link. Render on input, then present on the next display link tick (or immediately if idle long enough), and pause the link when there is no pending damage. This keeps keystroke latency low and idle CPU near zero.
- On a ProMotion XDR panel the refresh rate varies (up to 120 Hz); CADisplayLink exposes `preferredFrameRateRange`, which can request 120 Hz while typing (unsourced in this pass; check the CADisplayLink docs).
- Our 4.4 ms p95 is measured from key event to commit, not to glass. True on-glass latency adds compositor and scan-out time (at 120 Hz, up to roughly one or two refresh intervals of 8.3 ms).

### Gaps
- No source retrieved on measuring on-glass latency on macOS (high-speed camera or photodiode methods, such as Typometer-style tools or the Is It Snappy approach). Recommend a 240 fps phone camera test plus `CADisplayLink.targetTimestamp` logging as a cheap proxy; this is a suggestion, not a sourced practice.

## 5. App bundle and process model: one process per document, and the CLI on PATH

### Takeaway
Launch Services normally reuses a running instance and routes Finder and `open` file opens to it via `application:openURLs:`; it will not give us one process per document by default. Running the bundle's executable directly bypasses Launch Services (and `LSMultipleInstancesProhibited`). The common CLI pattern (VS Code) is a shell script in the bundle that runs the bundle's own Mach-O directly.

### Cited Findings
- Ken Thomases (cocoa-dev, 2011): Launch Services normally will not start a second instance, and Finder uses Launch Services on double-click ([cocoa-dev 2011](https://lists.apple.com/archives/cocoa-dev/2011/May/msg00261.html)).
- `LSMultipleInstancesProhibited` makes Launch Services deny a second instance "in any login session", which also blocked other users under fast user switching ([cocoa-dev 2007](https://lists.apple.com/archives/cocoa-dev/2007/Feb/msg00760.html)). Running the executable inside the bundle from Terminal bypasses Launch Services so the key does not apply ([macscripter thread](https://www.macscripter.net/t/how-does-one-prevent-launching-second-copy-of-an-app/71453)). Launch Services has a `kLSLaunchNewInstance` flag for forcing a new instance ([cocoa-dev 2005](https://lists.apple.com/archives/Cocoa-dev/2005/Aug/msg01576.html)); these sources are old.
- VS Code's `code` CLI is a bash script inside the bundle that resolves symlinks back to the `.app`, then runs `ELECTRON_RUN_AS_NODE=1 "$CONTENTS/MacOS/<name>" "$CONTENTS/Resources/app/out/cli.js" "$@"`, that is, it execs the bundle's own binary directly, in a non-GUI mode, as a CLI front end ([VS Code code.sh](https://raw.githubusercontent.com/microsoft/vscode/main/resources/darwin/bin/code.sh)).
- Zed ships a separate `cli` binary in `Zed.app/Contents/MacOS/cli`; "cli: install cli binary" symlinks it to `/usr/local/bin/zed`, with an alias fallback for non-admin users. Flags: `-n` new window, `-a` add to focused workspace, `-r` reuse window ([Zed macOS docs](https://zed.dev/docs/macos); [Zed CLI reference](https://zed.dev/docs/preview/reference/cli.html)).

### Inferences
- For one process per document from Finder: the running instance receives `application:openURLs:` and can spawn a sibling process (the bundle's own executable, with the path as an argument) per file, then hand activation to it (section 6). This keeps Launch Services' single registered instance as a thin router. Alternatively the router could use `NSWorkspace.openApplication(at:configuration:)` with `createsNewApplicationInstance = true` (the modern equivalent of `kLSLaunchNewInstance` and `open -n`); this property exists on `NSWorkspace.OpenConfiguration` but I did not source it in this pass.
- Exec'ing `Foo.app/Contents/MacOS/foo` directly normally still gives bundle identity, because `NSBundle.mainBundle` is derived from the executable's location inside the bundle (so Info.plist, icon and bundle ID apply and the Dock shows the app icon). The winit comment in section 6 reports activation issues only for unbundled binaries, which is consistent with this. Treat as inference until verified on our build.
- A CLI on PATH should be a symlink or tiny script pointing into the bundle (VS Code, Zed pattern), not a copy of the binary, so it always matches the installed bundle and resolves the bundle path via symlink resolution.

### Gaps
- Not checked: Sublime `subl`, BBEdit `bbedit`, Neovide CLI internals; whether VS Code's `cli.js` then spawns the GUI detached or uses `open`.
- No Apple doc fetched for `NSWorkspace.OpenConfiguration.createsNewApplicationInstance` or current Launch Services behaviour on macOS 15/26.

## 6. Window activation when launched from a terminal (macOS 14+ cooperative activation)

### Takeaway
Since macOS 14, `activateIgnoringOtherApps:` is on the deprecation path and `-[NSApplication activate]` is only a request that the system may refuse. Activation is cooperative: the currently active app should yield to the new one. Our observation (focused when the user launched it from their active terminal; not key when spawned by a background bench) matches this model.

### Cited Findings
- `activateIgnoringOtherApps:` is marked `API_DEPRECATED("This method will be deprecated in a future release. Use NSApp.activate instead.", macos(10.0, API_TO_BE_DEPRECATED))` (SDK: AppKit/NSApplication.h:222).
- `-activate` (macOS 14.0): "You shouldn't assume the app will be active immediately after sending this message. The framework also does not guarantee that the app will be activated at all. For cooperative activation, the other application should call -yieldActivationToApplication: or equivalent prior to the target application invoking -activate. Invoking -activate on an already-active application cancels any pending activation yields by the receiver." (SDK: AppKit/NSApplication.h:224-236; [Apple: activate()](https://developer.apple.com/documentation/appkit/nsapplication/activate().md))
- `yieldActivationToApplication:` (macOS 14.0) "will not deactivate the current app, nor will it activate the other app"; the target must later call `activate`. `yieldActivationToApplicationWithBundleIdentifier:` covers a target not yet running (SDK: AppKit/NSApplication.h:238-254; [Apple: yieldActivation(to:)](https://developer.apple.com/tutorials/data/documentation/appkit/nsapplication/yieldactivation(to:).md); [Apple: yieldActivation(toApplicationWithBundleIdentifier:)](https://developer.apple.com/tutorials/data/documentation/appkit/nsapplication/yieldactivation(toapplicationwithbundleidentifier:).md)).
- `NSRunningApplication.activateWithOptions:` returns whether the system allowed the request, and the yielding app should call yield before the target activates (SDK: AppKit/NSRunningApplication.h:138-148; [Apple: activate(from:options:)](https://developer.apple.com/tutorials/data/documentation/appkit/nsrunningapplication/activate(from:options:).md)).
- Wine's macOS 14 patch gates on `respondsToSelector:yieldActivationToApplication:` and notes yields must be issued in response to another app trying to activate, not preemptively ([Wine patch thread](https://list.winehq.org/hyperkitty/list/wine-gitlab@list.winehq.org/thread/K4XSASJC5FYH262CAN453KE4ZHNRCKDX/)).
- Electron uses `activate` on macOS 14+ and keeps `activateIgnoringOtherApps` on older systems ([Electron commit #42187](https://ayakael.net/mirrors/electron/commit/220166419f1591f61712c6a9e93d552f444789c8)). A Ghostty commit calls `activate(ignoringOtherApps: true)` when creating a window ([Ghostty commit 870b74f](https://git.uoc.run.place/Applied-Software/ghostty/commit/870b74f4da75ef2baa1f430602c901cb1405802e)).
- winit 0.24 (2020-era) documented that windows from unbundled apps (for example `cargo run`) can fail to activate and stay greyed out, that `activateIgnoringOtherApps` alone did not fix it, and worked around it by briefly activating the Dock and back ([winit 0.24 activation_hack.rs](https://docs.rs/crate/winit/0.24.0/source/src/platform_impl/macos/activation_hack.rs)). Old, but relevant if we ever run unbundled.

### Inferences
- A terminal cannot be made to call `yieldActivation` for us. When the user types the command, the terminal is active and the user interaction is recent, so the new app's `activate` is granted (what we saw). In a background bench the terminal is not frontmost, so the request is refused (what we saw). This is expected behaviour, not a bug. For benches, assert on window existence and drive input via events, or bring the window front via the bench harness itself.
- Where our own code launches the window process (CLI launcher, or the Finder router in section 5), the launcher should call `yieldActivationToApplication:` (or the bundle-ID variant if the child is not yet registered) before the child calls `activate`. A launcher that is a plain CLI process with no NSApplication cannot yield, so the child relies on the terminal being active.
- Call `activate` (macOS 14+) then `makeKeyAndOrderFront:`; fall back to `activateIgnoringOtherApps:YES` only on macOS 13 and earlier.

### Gaps
- The Apple article "Passing control from one app to another with cooperative activation" rendered empty when fetched, so its exact guidance (including any terminal-specific notes) is not quoted.
- No source on whether macOS 15/26 tightened activation further.

## 7. fork() without exec, and posix_spawn with POSIX_SPAWN_SETSID

### Takeaway
Confirmed long-standing rule: after `fork()` a child must not use CoreFoundation, Objective-C or anything above libSystem until it `exec`s. A launcher must not fork a window process; it should `posix_spawn` (or fork plus immediate exec) the bundle binary. `POSIX_SPAWN_SETSID` exists in the current macOS SDK and gives a detached session in one call.

### Cited Findings
- Apple TN2083, quoted in a Python bug: many Mac OS X frameworks do not work reliably if you call fork but do not call exec; the only exception is the System framework, and even there POSIX limits apply between fork and exec ([bugs.python.org msg304616](https://bugs.python.org/msg304616)).
- Calling any CoreFoundation API in the forked child is unsafe, and that extends to higher-level APIs since you cannot know whether they use CF internally; only async-signal-safe calls are allowed, so exec immediately ([darwin-dev 2009](https://lists.apple.com/archives/darwin-dev/2009/May/msg00019.html)). Mach ports are not inherited across fork, which is part of why ([Xcode-users 2008](https://lists.apple.com/archives/Xcode-users/2008/Apr/msg00243.html)). Break on `__THE_PROCESS_HAS_FORKED_AND_YOU_CANNOT_USE_THIS_COREFOUNDATION_FUNCTIONALITY___YOU_MUST_EXEC__` to find offenders ([gist](https://gist.github.com/meeech/1413770)). Cocoa cannot be reinitialised in a forked child ([Stack Overflow archive](https://kiwix.lema.org/stackoverflow.com_en_all_2023-11/questions/7379419/fork-cocoa-process-and-re-init-cocoa-how)). These sources are 2008 to 2017; the rule has not changed.
- The current SDK defines `POSIX_SPAWN_SETSID 0x0400` alongside `POSIX_SPAWN_SETPGROUP`, `POSIX_SPAWN_SETSIGDEF`, `POSIX_SPAWN_SETSIGMASK`, `POSIX_SPAWN_SETEXEC`, `POSIX_SPAWN_START_SUSPENDED` and `POSIX_SPAWN_CLOEXEC_DEFAULT` (SDK: usr/include/sys/spawn.h:45-62). Older Apple man pages do not list SETSID ([Apple archive man page](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/posix_spawnattr_setflags.3.html)), so it is a later addition.

### Inferences
- Recommended launcher: `posix_spawn` the bundle's executable with `POSIX_SPAWN_SETSID` (detaches from the terminal's session and job control, so closing the terminal or Ctrl-C does not kill the editor), `POSIX_SPAWN_CLOEXEC_DEFAULT` plus explicit file actions redirecting stdin/stdout/stderr to `/dev/null` or a log, and `POSIX_SPAWN_SETSIGDEF`/`SETSIGMASK` to reset signal state. Rust's `std::process::Command` uses posix_spawn on macOS where it can, but does not expose SETSID; `CommandExt::pre_exec` with `setsid()` forces the fork path, which is fine because only libSystem calls run before exec. Using libc `posix_spawn` directly with the flag is the cleanest option.
- Because the spawned child is the bundle binary run directly (not via Launch Services), the activation guidance in section 6 applies to it.

### Gaps
- Objective-C runtime fork safety (`OBJC_DISABLE_INITIALIZE_FORK_SAFETY`, introduced around macOS 10.13) was not sourced in this pass; it is commonly cited (for example in Python multiprocessing and Ruby issues) but should be confirmed before quoting.
- No source retrieved on the minimum macOS version that supports `POSIX_SPAWN_SETSID`; check the XNU source or header availability annotations if supporting older systems.
