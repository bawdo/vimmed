//! The macOS shell: start-up, the event handlers and presentation.
//!
//! Everything lives on the main thread in `STATE`. A key is handled,
//! painted and staged into the back IOSurface straight away; the display
//! link commits it on its next tick and pauses itself when nothing is
//! staged. Background workers wake the main queue through the `Waker`.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dispatch2::DispatchQueue;
use editor_core::editor::{Config, Editor, OpenRequest};
use editor_core::effects::{Effects, Redraw};
use editor_core::host::{Host, Waker, WindowArgs, WindowRequest};
use editor_core::input::{Input, SignalKind};
use editor_core::platform::Platform;
use editor_core::tokens::TokenId;
use editor_core::view_model::ViewSource;
use editor_render::damage::Damage;
use editor_render::fonts::FontSet;
use editor_render::layout_cosmic::CosmicLayout;
use editor_render::view::DocView;
use editor_render::view::RectPt;
use marks::bench::{BenchPlan, Progress};
use marks::names;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, Message, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEvent,
    NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem, NSScreen, NSTextInputContext, NSView,
    NSWindow, NSWindowStyleMask,
};
use objc2_core_graphics::CGColor;
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString, NSUserDefaults};
use objc2_quartz_core::{CALayer, CATransaction};

use crate::displaylink::Link;
use crate::fonts_ct::{MacFonts, cache_file};
use crate::glass::{KeyTimes, Tick, input_kind, key_values, kind_value, redraw_kind};
use crate::present::{IoSurface, Presenter};
use crate::startup::{FirstFrame, fit_content};
use crate::view::{Delegate, DocumentView};
use crate::{clipboard, input, router, signal};

/// The window's content size in points. It is the bench protocol's size
/// (Task 4), used for every window, clamped to the screen's visible frame.
/// TODO(config task): replace with `size.window.width` and
/// `size.window.height` tokens.
const WINDOW_PT: (f64, f64) = (900.0, 1000.0);
/// Exit code when no window can come up (R34: as the launcher's 220).
const EXIT_NOT_STARTED: i32 = 220;

struct State {
    editor: Editor,
    doc: DocView,
    presenter: Presenter<IoSurface>,
    window: Retained<NSWindow>,
    view: Retained<DocumentView>,
    layer: Retained<CALayer>,
    link: Link,
    /// Keys staged and waiting for the tick that commits them.
    keys: Vec<KeyTimes>,
    /// Size in points and scale the frame was last built for.
    geometry: (f64, f64, f64),
    first: FirstFrame,
    /// A decoded image was staged and waits for its commit.
    image_staged: bool,
    bench: Option<BenchPlan>,
    progress: Progress,
    /// The editor took text (`Editor::ime_active`) after the last input.
    ime_active: bool,
}

/// What `run` prepares before the toolkit starts.
struct Prepared {
    editor: Editor,
    fonts: FontSet,
    base_dir: PathBuf,
    waker: Waker,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static PREPARED: RefCell<Option<Prepared>> = const { RefCell::new(None) };
    static EXIT: Cell<Option<i32>> = const { Cell::new(None) };
    /// The key-down `interpretKeyEvents:` is handling, for its timing and
    /// for `doCommandBySelector:`.
    static IME_KEY: RefCell<Option<Retained<NSEvent>>> = const { RefCell::new(None) };
    /// What `sync_ime` asked of the input context (true: the caret moved,
    /// false: discard the composition), made once the state is released,
    /// since AppKit may call straight back into the view.
    static IME_CALL: RefCell<Option<(Retained<NSTextInputContext>, bool)>> =
        const { RefCell::new(None) };
    /// The caret on screen as of the last input, for a
    /// `firstRectForCharacterRange:` that arrives while the state is busy.
    static CARET_SCREEN: Cell<Option<NSRect>> = const { Cell::new(None) };
}

/// Runs `f` on the state unless it is missing or already borrowed (an
/// AppKit call made while handling something else can re-enter).
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

fn now_s() -> f64 {
    marks::now_ns() as f64 / 1e9
}

struct MacHost {
    waker: Waker,
}

impl Host for MacHost {
    fn clipboard_get(&mut self) -> Option<String> {
        clipboard::get()
    }
    fn clipboard_set(&mut self, text: &str) {
        clipboard::set(text);
    }
    /// `/usr/bin/open`, detached as a sibling window is, with the toolkit
    /// and `<ENV_PREFIX>*` variables stripped.
    fn open_external(&mut self, target: &str) {
        let strip = detach::sibling_strip_env(app_meta::ENV_PREFIX);
        let strip: Vec<&str> = strip.iter().map(String::as_str).collect();
        let args = [std::ffi::OsString::from(target)];
        let opener = std::path::Path::new("/usr/bin/open");
        match detach::spawn_detached(opener, &args, &strip) {
            Ok(pid) => detach::reap(pid),
            Err(e) => eprintln!("{}: could not open {target}: {e}", app_meta::SLUG),
        }
    }
    /// A sibling window process through `detach` (`router::spawn_sibling`:
    /// the current exe, its own session, stripped variables), tied to no
    /// launcher.
    fn spawn_window(&mut self, req: WindowRequest) {
        let args = router::sibling_args(&req.path, req.view, req.display_name.as_deref());
        router::spawn_and_yield(&req.path, &args);
    }
    fn now_ns(&self) -> u64 {
        marks::now_ns()
    }
    fn waker(&self) -> Waker {
        self.waker.clone()
    }
}

/// A waker that queues one `on_wake` on the main queue, however many
/// workers call it before it runs.
fn main_queue_waker() -> Waker {
    let queued = Arc::new(AtomicBool::new(false));
    Arc::new(move || {
        if !queued.swap(true, Ordering::AcqRel) {
            let queued = queued.clone();
            DispatchQueue::main().exec_async(move || {
                queued.store(false, Ordering::Release);
                on_wake();
            });
        }
    })
}

/// Opens the file, loads fonts and runs the application until the window
/// closes. Returns the exit code. With no path this instance is the router
/// (see `router`): no window, and exit 0 once its work is done.
pub fn run(args: WindowArgs) -> i32 {
    let Some(mtm) = MainThreadMarker::new() else {
        return EXIT_NOT_STARTED;
    };
    // Documents arrive through `application:openURLs:` only, never from argv.
    let key = NSString::from_str("NSTreatUnknownArgumentsAsOpen");
    let no = NSString::from_str("NO");
    let defaults = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[no.as_ref()]);
    // SAFETY: the dictionary maps NSString keys to property-list objects.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };

    let Some(path) = args.path else {
        router::set_router_mode(true);
        return run_app(mtm, None);
    };
    let waker = main_queue_waker();
    let host = MacHost {
        waker: waker.clone(),
    };
    let base_dir = path.parent().map(PathBuf::from).unwrap_or_default();
    let req = OpenRequest {
        path,
        view: args.view,
        display_name: args.display_name,
    };
    let editor = Editor::open(req, Config::default(), Platform::Mac, Box::new(host));
    let fonts = FontSet::with_lookup(Box::new(MacFonts::load()), cache_file(), waker.clone());
    marks::mark(names::FONTS_LOADED);
    PREPARED.with(|p| {
        *p.borrow_mut() = Some(Prepared {
            editor,
            fonts,
            base_dir,
            waker,
        })
    });
    run_app(mtm, Some(main_menu(mtm)))
}

/// Runs AppKit until `finish`. The router has no menu and keeps the
/// bundle's activation policy.
fn run_app(mtm: MainThreadMarker, menu: Option<Retained<NSMenu>>) -> i32 {
    let app = NSApplication::sharedApplication(mtm);
    if menu.is_some() {
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    }
    let delegate = Delegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.setMainMenu(menu.as_deref());
    app.run();
    // Drop the window and its surfaces before returning.
    STATE.with(|s| s.borrow_mut().take());
    drop(delegate);
    EXIT.with(Cell::get).unwrap_or(0)
}

/// Quit (Cmd+Q) and Close (Cmd+W), both routed to the editor.
fn main_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let item = |title: &str, action, key: &str| {
        // SAFETY: standard AppKit actions with their documented signatures.
        unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        }
    };
    let app_menu = NSMenu::new(mtm);
    app_menu.addItem(&item("Close", sel!(performClose:), "w"));
    app_menu.addItem(&item("Quit", sel!(terminate:), "q"));
    let top = item("", sel!(terminate:), "");
    top.setSubmenu(Some(&app_menu));
    let bar = NSMenu::new(mtm);
    bar.addItem(&top);
    bar
}

/// `applicationDidFinishLaunching:`: the window, the first paint and the
/// first staging. The first commit waits for the display link's first tick,
/// with the event loop live.
pub fn launch(delegate: &Delegate) {
    marks::mark(names::TOOLKIT_INIT);
    let mtm = MainThreadMarker::from(delegate);
    let Some(Prepared {
        editor,
        fonts,
        base_dir,
        waker,
    }) = PREPARED.with(|p| p.borrow_mut().take())
    else {
        return;
    };
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    // Fit inside the visible frame less the title bar, so AppKit doesn't
    // constrain (resize) the window as it goes on screen.
    let size = match NSScreen::mainScreen(mtm) {
        Some(screen) => {
            let max =
                NSWindow::contentRectForFrameRect_styleMask(screen.visibleFrame(), style, mtm);
            fit_content(WINDOW_PT, (max.size.width, max.size.height))
        }
        None => WINDOW_PT,
    };
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(size.0, size.1));
    // SAFETY: the designated initialiser with valid arguments.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: we keep our own reference, so AppKit must not release it.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(editor.title()));
    let tokens = editor.tokens().clone();
    let bg = tokens.color(TokenId::ColorBg);
    let c = |v: u8| f64::from(v) / 255.0;
    let bg = CGColor::new_srgb(c(bg.r), c(bg.g), c(bg.b), c(bg.a));
    let (view, layer) = DocumentView::new(mtm, rect, &bg);
    window.setContentView(Some(&view));
    window.center();
    marks::mark(names::WINDOW_CREATED);

    let scale = window.backingScaleFactor();
    layer.setContentsScale(scale);
    let layout = CosmicLayout::new(fonts, &tokens);
    let (w, h) = (size.0 as f32, size.1 as f32);
    let mut doc = DocView::new(Box::new(layout), &tokens, w, h, scale as f32);
    doc.set_images(&base_dir, waker.clone());
    doc.set_highlighting(waker);
    doc.paint_all(&editor);
    marks::mark(names::FIRST_PAINT);
    let frame = doc.frame();
    let mut presenter = Presenter::new(frame.width_px, frame.height_px);
    let d = doc.take_damage();
    presenter.stage(doc.frame(), &d);
    // SAFETY: the view implements `tick:`.
    let link = Link::new(&view, &view, DocumentView::tick_sel());

    let state = State {
        editor,
        doc,
        presenter,
        window: window.clone(),
        view: view.clone(),
        layer,
        link,
        keys: Vec::new(),
        geometry: (size.0, size.1, scale),
        first: FirstFrame::default(),
        image_staged: false,
        bench: BenchPlan::from_env(),
        progress: Progress {
            start_ns: marks::now_ns(),
            ..Progress::default()
        },
        ime_active: false,
    };
    let bench = state.bench.is_some();
    STATE.with(|s| *s.borrow_mut() = Some(state));
    // SIGTERM keeps a recovery copy of unsaved text; until now it ends
    // the process as by default.
    if let Err(e) = signal::install(on_terminate) {
        eprintln!(
            "{}: SIGTERM will not keep a recovery copy: {e}",
            app_meta::SLUG
        );
    }
    // A message the open left (an unreadable file) fades like any other.
    with_state(|st| {
        if let Some(at) = st.editor.open_effects().wake_at {
            arm_wake(st, at);
        }
    });

    window.setDelegate(Some(ProtocolObject::from_ref(delegate)));
    window.makeKeyAndOrderFront(None);
    window.makeFirstResponder(Some(&view));
    // The deprecated call on purpose: macOS 14's cooperative `activate()`
    // never takes focus from the active app, so a window the launcher
    // starts from a terminal would never come forward or become key (and
    // `first_frame` waits for key). winit and OpenJFX make the same call.
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    if let Some(screen) = window.screen() {
        marks::value(names::REFRESH_HZ, screen.maximumFramesPerSecond() as f64);
    }
    with_state(|st| st.link.resume());
    if window.isKeyWindow() {
        on_became_key();
    }
    if bench {
        schedule_bench_check();
    }
}

/// Applies the editor's effects and stages whatever they painted. Returns
/// the exit code when the editor asked to close.
fn apply(st: &mut State, fx: Effects) -> Option<i32> {
    if let Some(t) = &fx.title {
        st.window.setTitle(&NSString::from_str(t));
    }
    if let Some(d) = fx.dirty {
        st.window.setDocumentEdited(d);
    }
    // A reader mode change reaches the view before the scroll below, and
    // a full repaint comes with it; the input method follows in
    // `sync_ime`.
    if let Some(on) = fx.reader {
        st.doc.set_reader(on);
    }
    if let Some(at) = fx.wake_at {
        arm_wake(st, at);
    }
    let mut redraw = fx.redraw;
    if fx.scroll_to_cursor && st.doc.scroll_to_cursor(st.editor.cursor()) {
        redraw = Redraw::All;
    }
    if let Some(s) = fx.scroll
        && st.doc.scroll_by(s)
    {
        redraw = Redraw::All;
    }
    match redraw {
        Redraw::None => {}
        Redraw::All => st.doc.paint_all(&st.editor),
        Redraw::Lines(r) => st.doc.paint_lines(&st.editor, r),
        Redraw::Status => st.doc.paint_lines(&st.editor, 0..0),
    }
    stage(st);
    sync_ime(st);
    fx.close
}

/// Ticks the editor at `at` (`marks::now_ns` clock).
fn arm_wake(st: &State, at: u64) {
    let delay_s = at.saturating_sub(marks::now_ns()) as f64 / 1e9;
    st.view.schedule_wake(delay_s);
}

/// Keeps the input method in step with the editor: while text is being
/// composed the candidate window follows the caret, and a composition the
/// editor dropped (Esc out of Insert) is discarded.
fn sync_ime(st: &mut State) {
    let active = st.editor.ime_active();
    let composing = st.editor.preedit().is_some();
    if active {
        let rect = screen_rect(&st.view, st.doc.caret_rect());
        CARET_SCREEN.with(|c| c.set(Some(rect)));
    }
    if (composing || (st.ime_active && !active))
        && let Some(ctx) = st.view.inputContext()
    {
        IME_CALL.with(|c| *c.borrow_mut() = Some((ctx, composing)));
    }
    st.ime_active = active;
}

/// Makes the input context call `sync_ime` asked for. Call it once the
/// state is no longer borrowed.
fn flush_ime() {
    if let Some((ctx, moved)) = IME_CALL.with(|c| c.borrow_mut().take()) {
        if moved {
            ctx.invalidateCharacterCoordinates();
        } else {
            ctx.discardMarkedText();
        }
    }
}

/// Copies new damage into the back surface and wakes the display link.
fn stage(st: &mut State) -> bool {
    let d = st.doc.take_damage();
    if d.is_empty() {
        return false;
    }
    st.presenter.stage(st.doc.frame(), &d);
    st.link.resume();
    true
}

/// True when this key-down goes through the input method.
pub fn takes_ime(event: &NSEvent) -> bool {
    let mods = input::mods_from_flags(event.modifierFlags().0);
    with_state(|st| {
        let composing = st.editor.preedit().is_some();
        input::routes_to_ime(st.editor.ime_active(), composing, event.keyCode(), mods)
    })
    .unwrap_or(false)
}

pub fn begin_ime_key(event: &NSEvent) {
    IME_KEY.with(|k| *k.borrow_mut() = Some(event.retain()));
}

pub fn end_ime_key() {
    IME_KEY.with(|k| k.borrow_mut().take());
}

/// Text the input method committed, or its composition, timed from the
/// key-down that caused it (or from now, for a palette or a click).
pub fn on_ime(inp: Input) {
    let handler_s = now_s();
    let event_s = IME_KEY.with(|k| k.borrow().as_ref().map(|e| e.timestamp()));
    handle(inp, event_s.unwrap_or(handler_s), handler_s);
}

/// `doCommandBySelector:`: the input method passed its key on, so the key
/// goes to the editor as it is.
pub fn on_ime_command() {
    if let Some(event) = IME_KEY.with(|k| k.borrow_mut().take()) {
        on_key(&event);
    }
}

/// `unmarkText`: the composition is committed as typed.
pub fn on_unmark() {
    let handler_s = now_s();
    let event_s = IME_KEY.with(|k| k.borrow().as_ref().map(|e| e.timestamp()));
    let kind = input_kind(&Input::Text(String::new()));
    let event_s = event_s.unwrap_or(handler_s);
    run_input(kind, event_s, handler_s, |e| e.commit_preedit());
}

/// The composition's length in UTF-16 units, while there is one.
pub fn marked_utf16_len() -> Option<usize> {
    with_state(|st| st.editor.preedit().map(|(t, _)| t.encode_utf16().count())).flatten()
}

/// The caret in screen coordinates, for the candidate window. With no
/// caret in the document (the command line, a search), the bottom left of
/// the view, by the status bar.
pub fn caret_screen_rect(view: &NSView) -> NSRect {
    match with_state(|st| st.doc.caret_rect()) {
        Some(caret) => screen_rect(view, caret),
        // The state is busy (AppKit called back while handling an input).
        None => CARET_SCREEN
            .with(Cell::get)
            .unwrap_or_else(|| screen_rect(view, None)),
    }
}

/// `caret` (view points) on screen, or the view's bottom left without one.
fn screen_rect(view: &NSView, caret: Option<RectPt>) -> NSRect {
    let rect = match caret {
        Some(r) => NSRect::new(
            NSPoint::new(f64::from(r.x), f64::from(r.y)),
            NSSize::new(f64::from(r.w), f64::from(r.h)),
        ),
        None => NSRect::new(NSPoint::new(0.0, view.bounds().size.height), NSSize::ZERO),
    };
    let in_window = view.convertRect_toView(rect, None);
    match view.window() {
        Some(w) => w.convertRectToScreen(in_window),
        None => in_window,
    }
}

pub fn on_key(event: &NSEvent) {
    let handler_s = now_s();
    let Some(inp) = input::from_event(event) else {
        return;
    };
    handle(inp, event.timestamp(), handler_s);
}

/// One input to the editor, painted, staged and timed.
fn handle(inp: Input, event_s: f64, handler_s: f64) {
    run_input(input_kind(&inp), event_s, handler_s, |e| e.handle(inp));
}

/// Runs `f` on the editor as input of kind `input`, then paints, stages and
/// times it.
fn run_input(
    input: &'static str,
    event_s: f64,
    handler_s: f64,
    f: impl FnOnce(&mut Editor) -> Effects,
) {
    let close = with_state(|st| {
        let fx = f(&mut st.editor);
        let redraw = redraw_kind(&fx.redraw);
        let close = apply(st, fx);
        st.keys.push(KeyTimes {
            event_s,
            handler_s,
            staged_s: now_s(),
            input,
            redraw,
        });
        // A key that changed nothing still counts once the next tick runs.
        st.link.resume();
        close
    });
    flush_ime();
    if let Some(Some(code)) = close {
        finish(code);
    }
}

/// The display link tick: commit what is staged, log key timings and the
/// first frame, then pause until the next staging.
pub fn on_tick() {
    let tick_s = now_s();
    let bench_changed = with_state(|st| {
        let committed = st.presenter.commit(&st.layer);
        if committed {
            CATransaction::flush();
        }
        let done_s = now_s();
        let now_ns = marks::now_ns();
        let mut changed = false;
        if committed && st.first.tick_committed() {
            marks::mark(names::FIRST_FRAME);
            st.progress.first_frame_ns = Some(now_ns);
            // Lookups and the Core Text warm-up wait for the first frame.
            st.doc.start_font_fallback();
            // So does the file watch: never on the start-up path.
            st.editor.start_watching();
            changed = true;
        }
        if !st.keys.is_empty() {
            let t = Tick {
                tick_s,
                done_s,
                target_s: st.link.target_s(),
                period_s: st.link.period_s(),
            };
            for k in st.keys.drain(..) {
                for (name, v) in key_values(&k, &t) {
                    marks::value(name, v);
                }
                let (name, v) = kind_value(&k);
                marks::value(&name, v);
                st.progress.keys_done += 1;
            }
            st.progress.last_key_ns = Some(now_ns);
            changed = true;
        }
        if committed {
            changed |= image_shown(st, now_ns);
        }
        if !st.presenter.has_staged() {
            st.link.pause();
        }
        changed && st.bench.is_some()
    });
    if bench_changed == Some(true) {
        schedule_bench_check();
    }
}

/// After any commit: a staged decoded image is now on screen. Returns true
/// when that is news for the bench.
fn image_shown(st: &mut State, now_ns: u64) -> bool {
    if !st.image_staged {
        return false;
    }
    st.image_staged = false;
    st.progress.image_shown_ns.get_or_insert(now_ns);
    true
}

/// The window became key. While `first_frame` is still owed, stage the
/// whole frame so the next tick commits and can mark it. The editor hears
/// of the focus (its file watch looks at once); only what that changed is
/// applied, so the start-up path stages nothing more.
pub fn on_became_key() {
    let close = with_state(|st| {
        if st.first.became_key() {
            let frame = st.doc.frame();
            let mut all = Damage::new();
            all.add(0..frame.height_px);
            st.presenter.stage(frame, &all);
            st.link.resume();
        }
        let fx = st.editor.handle(Input::Focus(true));
        if fx == Effects::default() {
            return None;
        }
        apply(st, fx)
    });
    flush_ime();
    if let Some(Some(code)) = close {
        finish(code);
    }
}

/// A background worker finished, or a timer the editor asked for
/// (`Effects::wake_at`) fired: let the editor and the view take results.
pub fn on_wake() {
    let close = with_state(|st| {
        let fx = st.editor.handle(Input::Tick {
            now_ns: marks::now_ns(),
        });
        let close = apply(st, fx);
        let fonts = st.doc.poll_fonts(&st.editor);
        if st.doc.poll_async() && stage(st) {
            st.image_staged = true;
        } else if fonts {
            stage(st);
        }
        close
    });
    flush_ime();
    if let Some(Some(code)) = close {
        finish(code);
    }
}

/// The window moved, resized or changed scale: rebuild the frame and the
/// surfaces at the new size, and commit at once so the contents keep up
/// with a live resize.
pub fn on_geometry() {
    let changed = with_state(|st| {
        let size = st.view.bounds().size;
        let scale = st.window.backingScaleFactor();
        let g = (size.width, size.height, scale);
        if g == st.geometry {
            return false;
        }
        st.geometry = g;
        st.doc
            .resize(size.width as f32, size.height as f32, scale as f32);
        st.doc.paint_all(&st.editor);
        let frame = st.doc.frame();
        st.presenter = Presenter::new(frame.width_px, frame.height_px);
        st.layer.setContentsScale(scale);
        // Committed outside a tick: never marks `first_frame`.
        if stage(st) && st.presenter.commit(&st.layer) {
            return image_shown(st, marks::now_ns()) && st.bench.is_some();
        }
        false
    });
    if changed == Some(true) {
        schedule_bench_check();
    }
}

pub fn request_close() {
    if router::is_router_mode() {
        // No window and nothing unsaved: quitting (or logging out) is fine.
        finish(0);
        return;
    }
    let close = with_state(|st| {
        let fx = st.editor.handle(Input::CloseRequested);
        apply(st, fx)
    });
    flush_ime();
    if let Some(Some(code)) = close {
        finish(code);
    }
}

/// SIGTERM (`signal`): the editor writes its recovery copy, if the
/// document is dirty, and closes with 143.
fn on_terminate() {
    let term = SignalKind::Terminate;
    let close = with_state(|st| {
        let fx = st.editor.handle(Input::Signal(term));
        apply(st, fx)
    });
    flush_ime();
    finish(close.flatten().unwrap_or(term.exit_code()));
}

/// Stops the event loop; `run` then returns `code`. The lock record and
/// the file watch go first, so no way out (a bench deadline, the router,
/// the exit below) leaves them behind; so does the recovery copy of a way
/// out that did not close the editor (a signal's close kept its copy).
pub(crate) fn finish(code: i32) {
    EXIT.with(|e| e.set(Some(code)));
    with_state(|st| {
        st.editor.release_lock();
        st.editor.discard_recovery();
        st.editor.stop_watching();
        st.link.pause();
        st.window.orderOut(None);
    });
    let Some(mtm) = MainThreadMarker::new() else {
        std::process::exit(code);
    };
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    // `stop` takes effect after the next event, so send one.
    let wake = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        0,
        0,
    );
    if let Some(e) = wake {
        app.postEvent_atStart(&e, false);
    }
}

/// Re-checks the bench deadline after the time left until it.
fn schedule_bench_check() {
    let next = with_state(|st| {
        let plan = st.bench?;
        let left = plan.deadline(&st.progress).saturating_sub(marks::now_ns());
        Some((st.view.clone(), left as f64 / 1e9))
    });
    if let Some(Some((view, delay))) = next {
        view.schedule_bench_check(delay);
    }
}

/// A bench run exits by itself at its deadline (see `marks::bench`).
pub fn on_bench_check() {
    let due = with_state(|st| {
        let plan = st.bench?;
        (marks::now_ns() >= plan.deadline(&st.progress)).then(|| plan.exit_code(&st.progress))
    });
    match due {
        Some(Some(code)) => finish(code),
        Some(None) => schedule_bench_check(),
        None => {}
    }
}
