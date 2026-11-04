//! The Linux shell: start-up, the event handlers and presentation.
//!
//! Everything lives on the main thread in `STATE`. A key is handled and
//! painted into the frame, then the canvas is queued for drawing straight
//! away. Its snapshot re-uploads only the strips the damage touched; the
//! frame clock's `after-paint` closes the frame's timings. Background
//! workers wake the main loop through the `Waker`.
//!
//! Input methods: one `IMMulticontext` on the canvas. The key controller
//! feeds it only while the editor takes text (`sync_ime`), so in Normal and
//! Visual every key is mapped directly; its `commit` is `Input::Text` and
//! `preedit-changed` is `Input::Preedit`.

use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use editor_core::editor::{Config, Editor, OpenRequest};
use editor_core::effects::{Effects, Redraw};
use editor_core::host::{Host, Waker, WindowArgs, WindowRequest};
use editor_core::input::Input;
use editor_core::platform::Platform;
use editor_core::tokens::TokenId;
use editor_core::view_model::ViewSource;
use editor_render::fonts::FontSet;
use editor_render::layout_cosmic::CosmicLayout;
use editor_render::view::DocView;
use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;
use gtk::{gdk, glib};
use marks::bench::{BenchPlan, Progress};
use marks::names;

use crate::canvas::{self, Canvas};
use crate::clipboard::Clipboard;
use crate::fonts_fc::{LinuxFonts, cache_file};
use crate::strips::{self, StripSet};
use crate::timing::{self, FirstFrame, KeyTimes};
use crate::{env, input};

/// The window's default content size in points: the bench protocol's size
/// (Task 4), as on macOS.
/// TODO(config task): replace with `size.window.width` and
/// `size.window.height` tokens.
const WINDOW_PT: (i32, i32) = (900, 1000);
/// Exit code when no window can come up (R34: as the launcher's 220).
const EXIT_NOT_STARTED: i32 = 220;
/// Exit code for bad arguments, as the window's `main`.
const EXIT_USAGE: i32 = 2;
/// Presentation times arrive after the frame; they are polled this often...
const GLASS_POLL: Duration = Duration::from_millis(16);
/// ...this many times before the frame counts as never presented.
const GLASS_TRIES: u32 = 60;

/// A frame whose presentation time is owed to the log.
#[derive(Clone, Copy)]
enum Glass {
    /// `first_frame_glass`, as a mark.
    First,
    /// `key_glass_ms` for a key whose timing starts at this `now_ns`.
    Key(u64),
}

struct State {
    editor: Editor,
    doc: DocView,
    strips: StripSet<gdk::Texture>,
    window: gtk::Window,
    canvas: Canvas,
    clock: Option<gdk::FrameClock>,
    /// Width and height in points and the surface scale the frame was
    /// last built for.
    geometry: Option<(i32, i32, f64)>,
    first: FirstFrame,
    /// Keys handled and not yet closed by `after-paint`. A key whose
    /// `snap_end_ns` is still 0 has not been drawn.
    keys: Vec<KeyTimes>,
    /// (frame counter, what to log, polls so far).
    glass: Vec<(i64, Glass, u32)>,
    glass_polling: bool,
    glass_unavailable: bool,
    /// A decoded image damaged rows the next snapshot draws...
    image_damaged: bool,
    /// ...and that snapshot drew them: shown at the next `after-paint`.
    image_drawn: bool,
    bench: Option<BenchPlan>,
    progress: Progress,
    clipboard: Rc<OnceCell<Clipboard>>,
    key_ctl: gtk::EventControllerKey,
    im: gtk::IMMulticontext,
    /// The key controller feeds `im` (the editor takes text).
    ime_on: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static EXIT: Cell<Option<i32>> = const { Cell::new(None) };
    static MAIN_LOOP: glib::MainLoop = glib::MainLoop::new(None, false);
    /// The last key press's GDK time, taken in the capture phase: the input
    /// method's signals carry none.
    static KEY_MS: Cell<u32> = const { Cell::new(0) };
}

/// Runs `f` on the state unless it is missing or already borrowed (a GTK
/// call made while handling something else can re-enter).
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

struct GtkHost {
    waker: Waker,
    clipboard: Rc<OnceCell<Clipboard>>,
}

impl Host for GtkHost {
    fn clipboard_get(&mut self) -> Option<String> {
        self.clipboard.get()?.get()
    }
    fn clipboard_set(&mut self, text: &str) {
        if let Some(c) = self.clipboard.get() {
            c.set(text);
        }
    }
    fn open_external(&mut self, target: &str) {
        let _ = env::child_command("xdg-open").arg(target).spawn();
    }
    /// A sibling window process, detached as the launcher starts one, with
    /// the toolkit and `<ENV_PREFIX>*` variables stripped.
    fn spawn_window(&mut self, req: WindowRequest) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut args = Vec::new();
        if req.view {
            args.push(OsString::from("--view"));
        }
        if let Some(name) = &req.display_name {
            args.push(OsString::from("--display-name"));
            args.push(OsString::from(name));
        }
        args.push(OsString::from("--"));
        args.push(req.path.into_os_string());
        let strip = detach::sibling_strip_env(app_meta::ENV_PREFIX);
        let strip: Vec<&str> = strip.iter().map(String::as_str).collect();
        if let Err(e) = detach::spawn_detached(&exe, &args, &strip) {
            eprintln!("{}: could not start a window: {e}", app_meta::SLUG);
        }
    }
    fn now_ns(&self) -> u64 {
        marks::now_ns()
    }
    fn waker(&self) -> Waker {
        self.waker.clone()
    }
}

/// A waker that queues one `on_wake` on the main loop, however many
/// workers call it before it runs.
fn main_loop_waker() -> Waker {
    let queued = Arc::new(AtomicBool::new(false));
    Arc::new(move || {
        if !queued.swap(true, Ordering::AcqRel) {
            let queued = queued.clone();
            glib::idle_add_once(move || {
                queued.store(false, Ordering::Release);
                on_wake();
            });
        }
    })
}

/// Opens the file, loads fonts and runs GTK until the window closes.
/// Returns the exit code.
pub fn run(args: WindowArgs) -> i32 {
    let var_set = |k: &str| std::env::var_os(k).is_some();
    if env::default_vulkan_off(var_set(env::GSK_RENDERER), var_set(env::GDK_DISABLE)) {
        // SAFETY: `set_var` is unsound only while another thread reads the
        // environment. This runs before anything here starts a thread: the
        // window's `main` only parses arguments and opens the marks log, and
        // the editor (whose workers are the first threads) is opened below.
        unsafe { std::env::set_var(env::GDK_DISABLE, env::DISABLE_VULKAN) };
    }
    let Some(path) = args.path else {
        // The window's `main` refuses this first: Linux has no router.
        eprintln!("{}: no file given", app_meta::SLUG);
        return EXIT_USAGE;
    };
    let waker = main_loop_waker();
    let clipboard: Rc<OnceCell<Clipboard>> = Rc::default();
    let host = GtkHost {
        waker: waker.clone(),
        clipboard: clipboard.clone(),
    };
    let base_dir = path.parent().map(PathBuf::from).unwrap_or_default();
    let req = OpenRequest {
        path,
        view: args.view,
        display_name: args.display_name,
    };
    let editor = Editor::open(req, Config::default(), Platform::Linux, Box::new(host));
    let tokens = editor.tokens().clone();
    let resolver = LinuxFonts::load(
        tokens.family(TokenId::FontBody),
        tokens.family(TokenId::FontMono),
    );
    let fonts = FontSet::with_lookup(Box::new(resolver), cache_file(), waker.clone());
    marks::mark(names::FONTS_LOADED);

    if let Err(e) = gtk::init() {
        eprintln!("{}: GTK could not start: {e}", app_meta::SLUG);
        return EXIT_NOT_STARTED;
    }
    marks::mark(names::TOOLKIT_INIT);

    let window = gtk::Window::new();
    window.set_title(Some(editor.title()));
    window.set_default_size(WINDOW_PT.0, WINDOW_PT.1);
    let canvas: Canvas = glib::Object::new();
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    window.set_child(Some(&canvas));
    marks::mark(names::WINDOW_CREATED);
    let _ = clipboard.set(Clipboard::new(&WidgetExt::display(&window)));

    // The real size and scale are only known at the first snapshot, which
    // resizes and paints the whole frame.
    let layout = CosmicLayout::new(fonts, &tokens);
    let (w, h) = (WINDOW_PT.0 as f32, WINDOW_PT.1 as f32);
    let mut doc = DocView::new(Box::new(layout), &tokens, w, h, 1.0);
    doc.set_images(&base_dir, waker.clone());
    doc.set_highlighting(waker);
    let strip_px = std::env::var(format!("{}STRIP_PX", app_meta::ENV_PREFIX)).ok();
    let keys = gtk::EventControllerKey::new();
    let im = gtk::IMMulticontext::new();
    im.set_client_widget(Some(&canvas));

    let state = State {
        editor,
        doc,
        strips: StripSet::new(strips::strip_px(strip_px.as_deref())),
        window: window.clone(),
        canvas: canvas.clone(),
        clock: None,
        geometry: None,
        first: FirstFrame::default(),
        keys: Vec::new(),
        glass: Vec::new(),
        glass_polling: false,
        glass_unavailable: false,
        image_damaged: false,
        image_drawn: false,
        bench: BenchPlan::from_env(),
        progress: Progress {
            start_ns: marks::now_ns(),
            ..Progress::default()
        },
        clipboard,
        key_ctl: keys.clone(),
        im: im.clone(),
        ime_on: false,
    };
    let bench = state.bench.is_some();
    STATE.with(|s| *s.borrow_mut() = Some(state));
    connect(&window, &canvas, &keys, &im);
    // An editor that opens taking text (vim off, bench mode) feeds the
    // input method from the first key.
    with_state(sync_ime);
    // A message the open left (an unreadable file) fades like any other.
    if let Some(Some(at)) = with_state(|st| st.editor.open_effects().wake_at) {
        arm_wake(at);
    }
    window.present();
    if bench {
        schedule_bench_check();
    }
    MAIN_LOOP.with(|l| l.run());
    STATE.with(|s| s.borrow_mut().take());
    window.destroy();
    EXIT.with(Cell::get).unwrap_or(0)
}

fn connect(
    window: &gtk::Window,
    canvas: &Canvas,
    keys: &gtk::EventControllerKey,
    im: &gtk::IMMulticontext,
) {
    keys.connect_key_pressed(|c, key, _code, state| on_key(c, key, state));
    window.add_controller(keys.clone());
    let stamp = gtk::EventControllerKey::new();
    stamp.set_propagation_phase(gtk::PropagationPhase::Capture);
    stamp.connect_key_pressed(|c, _, _, _| {
        KEY_MS.with(|k| k.set(c.current_event_time()));
        glib::Propagation::Proceed
    });
    window.add_controller(stamp);

    im.connect_commit(|_, text| on_ime(Input::Text(text.to_owned())));
    im.connect_preedit_changed(|im| {
        let (text, _, cursor) = im.preedit_string();
        let cursor = input::char_to_byte(&text, cursor);
        let text = text.to_string();
        on_ime(Input::Preedit { text, cursor });
    });

    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter(|_| {
        with_state(|st| {
            if let Some(c) = st.clipboard.get() {
                c.refresh();
            }
            if st.ime_on {
                st.im.focus_in();
            }
        });
    });
    focus.connect_leave(|_| {
        with_state(|st| st.im.focus_out());
    });
    window.add_controller(focus);

    window.connect_close_request(|_| {
        request_close();
        glib::Propagation::Stop
    });

    canvas.connect_realize(|w| {
        if let Some(clock) = w.frame_clock() {
            clock.connect_after_paint(on_after_paint);
            with_state(|st| st.clock = Some(clock));
        }
        if let Some(surface) = w.native().and_then(|n| n.surface()) {
            surface.connect_scale_notify(|_| {
                with_state(|st| st.canvas.queue_draw());
            });
        }
    });
}

/// The surface's fractional scale (`GdkSurface:scale`, GTK 4.12 and later),
/// not the integer `scale_factor`.
fn surface_scale(w: &gtk::Widget) -> f64 {
    let Some(surface) = w.native().and_then(|n| n.surface()) else {
        return 1.0;
    };
    if gtk::check_version(4, 12, 0).is_none() {
        surface.scale()
    } else {
        f64::from(surface.scale_factor())
    }
}

/// Rebuilds the frame when the widget's size or the surface's scale moved.
fn sync_geometry(st: &mut State, w: &gtk::Widget) {
    let g = (w.width(), w.height(), surface_scale(w));
    if st.geometry == Some(g) {
        return;
    }
    st.geometry = Some(g);
    st.doc.resize(g.0 as f32, g.1 as f32, g.2 as f32);
    st.doc.paint_all(&st.editor);
}

/// The canvas snapshot: bring the strips up to date and append them.
pub fn on_snapshot(w: &gtk::Widget, snapshot: &gtk::Snapshot) {
    let start = marks::now_ns();
    with_state(|st| {
        sync_geometry(st, w);
        let d = st.doc.take_damage();
        st.strips.sync(st.doc.frame(), &d);
        canvas::append(snapshot, &st.strips, st.doc.frame());
        let end = marks::now_ns();
        if st.first.snapshot() {
            marks::mark(names::FIRST_PAINT);
        }
        for k in st.keys.iter_mut().filter(|k| k.snap_end_ns == 0) {
            k.snap_start_ns = start;
            k.snap_end_ns = end;
        }
        if std::mem::take(&mut st.image_damaged) {
            st.image_drawn = true;
        }
    });
}

/// Applies the editor's effects and paints what they changed into the
/// frame. Returns the exit code when the editor asked to close.
fn apply(st: &mut State, fx: Effects) -> Option<i32> {
    if let Some(t) = &fx.title {
        st.window.set_title(Some(t.as_str()));
    }
    // A reader mode change reaches the view before the scroll below, and
    // a full repaint comes with it. The dirty state shows in the title
    // and status bar; GTK has no edited flag.
    if let Some(on) = fx.reader {
        st.doc.set_reader(on);
    }
    if let Some(at) = fx.wake_at {
        arm_wake(at);
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
    sync_ime(st);
    fx.close
}

/// Ticks the editor at `at` (`marks::now_ns` clock). Millisecond
/// resolution: never tick before it is due.
fn arm_wake(at: u64) {
    let ns = at.saturating_sub(marks::now_ns());
    let delay = Duration::from_nanos(ns) + Duration::from_millis(1);
    glib::timeout_add_local_once(delay, on_wake);
}

/// Keeps the input method in step with the editor: the key controller
/// feeds it only while the editor takes text, and while text is being
/// composed its candidate window follows the caret.
fn sync_ime(st: &mut State) {
    let on = st.editor.ime_active();
    let turned_on = on && !st.ime_on;
    if on != st.ime_on {
        st.ime_on = on;
        if on {
            st.key_ctl.set_im_context(Some(&st.im));
            st.im.focus_in();
        } else {
            st.key_ctl.set_im_context(None::<&gtk::IMContext>);
            st.im.reset();
            st.im.focus_out();
        }
    }
    let composing = st.editor.preedit().is_some();
    if (turned_on || composing)
        && let Some(r) = st.doc.caret_rect()
    {
        let (w, h) = (r.w.ceil() as i32, r.h.ceil() as i32);
        let area = gdk::Rectangle::new(r.x as i32, r.y as i32, w, h);
        st.im.set_cursor_location(&area);
    }
}

fn on_key(
    c: &gtk::EventControllerKey,
    key: gdk::Key,
    state: gdk::ModifierType,
) -> glib::Propagation {
    let handler_ns = marks::now_ns();
    let k = KeyTimes::new(c.current_event_time(), handler_ns);
    let mods = input::mods_from_state(state.bits());
    let Some(inp) = input::map(key.into_glib(), key.to_unicode(), mods, k.start_ns) else {
        return glib::Propagation::Proceed;
    };
    handle(inp, k);
    glib::Propagation::Stop
}

/// Text the input method committed, or its composition, timed from the key
/// press that caused it (with none, from the handler).
fn on_ime(inp: Input) {
    let handler_ns = marks::now_ns();
    let event_ms = KEY_MS.with(Cell::get);
    handle(inp, KeyTimes::new(event_ms, handler_ns));
}

/// One input to the editor, painted, queued for drawing and timed.
fn handle(inp: Input, mut k: KeyTimes) {
    k.input = timing::input_kind(&inp);
    let close = with_state(|st| {
        let fx = st.editor.handle(inp);
        k.redraw = timing::redraw_kind(&fx.redraw);
        let close = apply(st, fx);
        // Straight to the frame clock: no idle or timeout hop. A key that
        // changed nothing is still drawn once, so its timings close.
        st.canvas.queue_draw();
        k.handled_ns = marks::now_ns();
        st.keys.push(k);
        close
    });
    if let Some(Some(code)) = close {
        finish(code);
    }
}

/// The frame clock painted a frame: log the first frame and key timings,
/// and note which frames owe a presentation time.
fn on_after_paint(clock: &gdk::FrameClock) {
    let now = marks::now_ns();
    let frame = clock.frame_counter();
    let bench_changed = with_state(|st| {
        let mut changed = false;
        if st.first.after_paint() {
            marks::mark(names::FIRST_FRAME);
            st.progress.first_frame_ns = Some(now);
            // Fallback lookups wait for the first frame.
            st.doc.start_font_fallback();
            st.glass.push((frame, Glass::First, 0));
            log_refresh(st);
            changed = true;
        }
        let (drawn, waiting): (Vec<KeyTimes>, Vec<KeyTimes>) =
            st.keys.drain(..).partition(|k| k.snap_end_ns != 0);
        st.keys = waiting;
        for k in drawn {
            for (name, v) in timing::key_values(&k, now) {
                marks::value(name, v);
            }
            if let Some((name, v)) = timing::kind_value(&k) {
                marks::value(&name, v);
            }
            st.glass.push((frame, Glass::Key(k.start_ns), 0));
            st.progress.keys_done += 1;
            st.progress.last_key_ns = Some(now);
            changed = true;
        }
        if std::mem::take(&mut st.image_drawn) {
            st.progress.image_shown_ns.get_or_insert(now);
            changed = true;
        }
        changed && st.bench.is_some()
    });
    poll_glass();
    if bench_changed == Some(true) {
        schedule_bench_check();
    }
}

fn log_refresh(st: &State) {
    let surface = st.window.surface();
    let display = WidgetExt::display(&st.window);
    let monitor = surface.and_then(|s| display.monitor_at_surface(&s));
    if let Some(hz) = monitor.and_then(|m| timing::refresh_hz(m.refresh_rate())) {
        marks::value(names::REFRESH_HZ, hz);
    }
}

/// Logs every owed presentation time that has arrived, and polls again
/// later while any are still owed (the frame clock may go idle).
fn poll_glass() {
    let again = with_state(|st| {
        let Some(clock) = st.clock.clone() else {
            st.glass.clear();
            return false;
        };
        for (frame, what, tries) in std::mem::take(&mut st.glass) {
            let timings = clock.timings(frame);
            let complete = timings.as_ref().is_some_and(|t| t.is_complete());
            if !complete && timings.is_some() && tries + 1 < GLASS_TRIES {
                st.glass.push((frame, what, tries + 1));
                continue;
            }
            let shown_us = timings
                .filter(|_| complete)
                .map_or(0, |t| t.presentation_time());
            log_glass(st, what, shown_us);
        }
        let again = !st.glass.is_empty() && !st.glass_polling;
        st.glass_polling |= again;
        again
    });
    if again == Some(true) {
        glib::timeout_add_local_once(GLASS_POLL, || {
            with_state(|st| st.glass_polling = false);
            poll_glass();
        });
    }
}

fn log_glass(st: &mut State, what: Glass, shown_us: i64) {
    let start = match what {
        Glass::First => 0,
        Glass::Key(start) => start,
    };
    let Some(ms) = timing::glass_ms(start, shown_us) else {
        if !std::mem::replace(&mut st.glass_unavailable, true) {
            marks::value(names::GLASS_UNAVAILABLE, 1.0);
        }
        return;
    };
    match what {
        Glass::First => {
            let shown_ns = (ms * 1e6) as u64;
            marks::value(names::FIRST_FRAME_GLASS, marks::ms_since_t0(shown_ns));
        }
        Glass::Key(_) => marks::value(names::KEY_GLASS_MS, ms),
    }
}

/// A background worker finished, or a timer the editor asked for
/// (`Effects::wake_at`) fired: let the editor and the view take results.
fn on_wake() {
    let close = with_state(|st| {
        let fx = st.editor.handle(Input::Tick {
            now_ns: marks::now_ns(),
        });
        let close = apply(st, fx);
        st.doc.poll_fonts(&st.editor);
        if st.doc.poll_async() {
            st.image_damaged = true;
        }
        st.canvas.queue_draw();
        close
    });
    if let Some(Some(code)) = close {
        finish(code);
    }
}

fn request_close() {
    let close = with_state(|st| {
        let fx = st.editor.handle(Input::CloseRequested);
        let close = apply(st, fx);
        st.canvas.queue_draw();
        close
    });
    if let Some(Some(code)) = close {
        finish(code);
    }
}

/// Stops the main loop; `run` then returns `code`.
fn finish(code: i32) {
    EXIT.with(|e| e.set(Some(code)));
    with_state(|st| st.window.set_visible(false));
    MAIN_LOOP.with(|l| l.quit());
}

/// Re-checks the bench deadline after the time left until it.
fn schedule_bench_check() {
    let left = with_state(|st| {
        let plan = st.bench?;
        Some(plan.deadline(&st.progress).saturating_sub(marks::now_ns()))
    });
    if let Some(Some(ns)) = left {
        // Timeouts have millisecond resolution: never wake before the
        // deadline and spin.
        let delay = Duration::from_nanos(ns) + Duration::from_millis(1);
        glib::timeout_add_local_once(delay, on_bench_check);
    }
}

/// A bench run exits by itself at its deadline (see `marks::bench`).
fn on_bench_check() {
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
