//! The editor facade: the one type a shell drives. It owns the buffer, the
//! block structure, the cursor and the mode, turns `Input` into edits and
//! `Effects`, and implements `ViewSource` for the renderer.
//!
//! The insert path: typing, Backspace, Delete, Enter, Tab and Shift+Tab
//! (which continue, indent and outdent list items and close fences, else
//! split the line or put in a tab: `list_keys`), the arrows, Home and End,
//! and dirty tracking. Task toggles (`gx`, Ctrl+Space) are in `list_keys`
//! too.
//!
//! With `Config::vim` the editor starts in Normal and routes Normal-mode
//! keys to the vim engine (`crate::vim`), which decides what they mean;
//! this file only carries out the `Step` it returns. Operators live in
//! `vim::operator`; every edit, typed or from an operator, goes through
//! `EditCtx::replace`, which records it in the history.
//!
//! Visual and Visual Line route their keys to the engine the same way; the
//! editor keeps the selection's anchor. The clipboard keys (Primary+C, X
//! and V, and Ctrl+C) are handled here before any mode sees the key.
//!
//! Search (`/` and `?`) is typed here: every key searches again from where
//! it started and moves the cursor to the match, Enter keeps it and Esc
//! puts the cursor back. The confirmed search stays for `n`, `N` and the
//! highlights, which follow every edit (`crate::search`).
//!
//! The command line (`:`), what it runs and `:s///c`'s Confirm mode live
//! in `cmdline`. `:w`, `:e!` and closing reach the file through a
//! `Saver` (`crate::saver`), which `open` sets up on the file it read
//! (`crate::file`). Closing with unsaved changes asks first (`prompt`).
//!
//! The status bar: `ViewSource::status` picks what its left side shows (a
//! message, then a prompt, then the line being typed, then the mode).
//! Messages fade after `status::MESSAGE_FOR_NS` (asked for with
//! `Effects::wake_at`, so the shell ticks then) and Esc clears one.
//! `handle` repaints the status bar whenever its model changed, and the
//! current match's lines whenever it moved, so no path has to ask.
//!
//! Undo groups: each Normal- or Visual-mode command is one undo step, a
//! paste is one, each `:s` is one (a whole Confirm session included), and
//! an Insert
//! session is one step from its entry key (with the line `o` opens or the
//! text `c` deletes) to Esc. An arrow key inside the session ends the step
//! and starts another, as in vim.
//!
//! Input methods (`ime`): committed text and the composition, which is
//! held beside the buffer and drawn at the caret until it commits.
//!
//! Reader view and the help overlay (`reader`): Primary+Shift+R (or
//! `--view`) hides the caret and makes the document
//! read-only, with keys that scroll the view; `:help` shows the cheat
//! sheet over the document until Esc.
//!
//! Links (`link`): `open_link` opens a link's target through the host, and
//! Primary+L opens the command line as the `link: ` prompt.
//!
//! The lock record (`locking`): opening a file takes one, another window
//! holding it asks the already-open prompt, and its `o` answer makes the
//! document read-only, refusing edits as reader view does.
//!
//! Tables (`table_keys`): Tab, Shift+Tab and Insert-mode Enter move
//! between cells, the `t` commands edit rows and columns, and `:table`
//! inserts a table or opens the grid picker, which takes every key while
//! it shows.
//!
//! External changes (`disk`): once the shell has drawn the first frame, a
//! watch on the file reloads a clean document that changed on disk and
//! warns about one with unsaved changes, a file gone, or one that cannot
//! be read or written.
//!
//! Autosave (`autosave`, `Config::autosave`): a second after the last
//! change the tick writes down `:w`'s path, quietly, unless something
//! holds it. `handle` reports each change of the dirty state once
//! (`Effects::dirty`); the shells show it beside the title.
//!
//! Recovery copies (`recovering`): while the document is dirty a copy of
//! its text is kept in the state directory, written on a worker; a signal
//! writes it at once and closes with 128 plus the signal's number. At
//! open, a copy a window that is gone left asks the recovery prompt.
//!
//! Vim off (`nonvim_keys`, with `crate::nonvim` saying what each key
//! means): the editor rests in Insert with no mode shown, Shift and the
//! arrows select, typing goes in runs that are one undo step each, and the
//! formatting, undo, clipboard and find chords act. Primary+F opens the
//! find bar in the status row, which takes every key while it shows.
//! Primary+S over a file changed on disk asks rather than refusing.
//!
//! The mouse (`mouse`): clicks place the cursor (reveal following on the
//! button's release), drags and double-clicks select, and clicks open
//! links, toggle task checkboxes and pick in the table picker, from the
//! hit the shell's renderer found. The wheel scrolls the view.
//!
//! Every position that comes from the user goes through `Buffer::clamp` or
//! `grapheme::next` / `grapheme::prev` before it reaches `Buffer::replace`,
//! so no edit splits a grapheme cluster.

mod appearance;
mod autosave;
mod cmdline;
mod disk;
mod ime;
mod link;
mod list_keys;
mod locking;
mod mouse;
mod nonvim_keys;
mod prompt;
mod reader;
mod recovering;
mod table_keys;

use std::ops::Range;
use std::path::PathBuf;

use crate::buffer::{Buffer, Pos, ShiftMark};
pub use crate::config::{Config, ThemePref};
use crate::effects::{Effects, Redraw};
use crate::ex::ExCmd;
use crate::file::lock::Lock;
use crate::file::recovery::{Found, Recovery};
use crate::file::{self, DiskEvent, Loaded, RegisterStore, StateDir, Watcher};
use crate::grapheme;
use crate::host::Host;
use crate::input::{Input, Key, KeyEvent, Mods, MouseEvent, MouseKind};
use crate::markdown::{
    BlockId, Blocks, InlineKind, LineDisplay, LineKind, Prefix, Reveal, SpanBase, TableId, code_bg,
    line_display, ordered_display, parse_line,
};
use crate::nonvim::NonVim;
use crate::platform::Platform;
use crate::saver::{FileSaver, Saver, Unwired};

use crate::search::{self, Hit, Search};
use crate::status::{self, Mode};
use crate::strings;
use crate::substitute::ConfirmSession;
use crate::tables::TablePicker;
use crate::tokens::{Theme, Tokens};
use crate::undo::History;
use crate::view_model::{
    CaretShape, LineShifts, MsgKind, Selection, StatusLeft, StatusModel, ViewSource,
};
use crate::vim::operator::{self, EditCtx, ListKey, OpOutcome, OpRange, Operator, Touched};
use crate::vim::register::{Register, Registers};
use crate::vim::repeat::Change;
use crate::vim::{self, Cursor, Step, Vim, Want, textobj, visual};
use prompt::{Ask, Prompt};

/// What to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub path: PathBuf,
    /// True to open in reader mode.
    pub view: bool,
    /// The name to show in place of the file name.
    pub display_name: Option<String>,
}

/// A search being typed, and what to go back to when it ends.
struct Searching {
    search: Search,
    /// The mode `/` or `?` came from: Normal, Visual or Visual Line.
    ret: Mode,
    /// The operator waiting for the match (`d/foo`).
    op: Option<Operator>,
    /// Which match counts (`2/foo`).
    count: usize,
    /// The Up/Down goal column before the search, for Esc to restore.
    goal: Option<Want>,
    /// The named register the waiting operator writes (`"ad/foo`).
    register: Option<char>,
}

/// A run of lines that reveals as a whole: a fenced code block or a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Code(BlockId),
    Table(TableId),
}

/// True when the bench variable (`<P>BENCH`) is set and not empty.
pub fn bench_enabled() -> bool {
    std::env::var_os(format!("{}BENCH", app_meta::ENV_PREFIX)).is_some_and(|v| !v.is_empty())
}

/// The start of the first bold's content on a paragraph line, if any.
fn first_bold(buf: &Buffer, blocks: &Blocks) -> Option<Pos> {
    (0..buf.line_count()).find_map(|line| {
        let kind = blocks.kind(line);
        if *kind != LineKind::Paragraph {
            return None;
        }
        parse_line(buf.line(line), kind)
            .into_iter()
            .find(|e| matches!(e.kind, InlineKind::Bold))
            .map(|e| Pos {
                line,
                col: e.content.start,
            })
    })
}

pub struct Editor {
    buf: Buffer,
    blocks: Blocks,
    /// The token set for `theme`, from the config (`appearance`).
    tokens: Tokens,
    /// The theme painted: the config's, or the system's appearance under
    /// `ThemePref::System`.
    theme: Theme,
    cursor: Pos,
    /// The column Up and Down (and `j`, `k`) aim for while they repeat.
    /// `None` means the cursor's own display column.
    goal: Option<Want>,
    mode: Mode,
    /// The selection's anchor while Visual is up; the cursor is its head.
    anchor: Option<Pos>,
    /// The command line's text (Command mode).
    cmdline: String,
    /// The selection `:` left Visual with.
    last_visual: Option<Selection>,
    /// The `:s///c` being answered (Confirm mode).
    confirm: Option<ConfirmSession>,
    /// The search being typed (Search mode).
    searching: Option<Searching>,
    /// The last search confirmed: what `n` and `N` look for and the
    /// highlights show.
    search: Option<Search>,
    vim: Vim,
    registers: Registers,
    /// The named register the command being carried out uses (`"a`).
    register: Option<char>,
    history: History,
    dirty: bool,
    /// Reader view (`reader`): no caret, read-only.
    reader: bool,
    /// Where reader view goes back to when it is toggled off.
    reader_return: Option<reader::Return>,
    /// A `g` reader view holds, waiting to see whether `gg` follows.
    reader_g: bool,
    /// The help overlay's first row, while `:help` shows it.
    help: Option<usize>,
    title: String,
    message: Option<(String, MsgKind)>,
    /// When the message goes (`marks::now_ns` clock); `None` keeps it until
    /// it is replaced or cleared.
    message_until: Option<u64>,
    /// Warnings about the file as it was read (not valid UTF-8), for the
    /// status bar. What the file watch finds is `disk`; `shown_warnings`
    /// puts them together.
    warnings: Vec<&'static str>,
    cfg: Config,
    platform: Platform,
    host: Box<dyn Host>,
    saver: Box<dyn Saver>,
    /// The text an input method is composing and its cursor (`ime`).
    preedit: Option<(String, usize)>,
    /// The question Prompt mode asks (`prompt`).
    asking: Option<Prompt>,
    /// The file the document is from: an empty path for an unnamed
    /// buffer. Links resolve against its directory (`link`).
    path: PathBuf,
    /// The link prompt, while the command line is asking for a link
    /// (`link`).
    linking: Option<link::Linking>,
    /// The target the open-link prompt would hand the system handler,
    /// while it asks (`link`).
    opening: Option<String>,
    /// This window's lock record on the file, while it is open
    /// (`locking`).
    lock: Option<Lock>,
    /// `o` at the already-open prompt: edits and writes are refused
    /// (`locking`).
    read_only_chosen: bool,
    /// Why the lock record could not be written, held back while the
    /// already-open prompt asks (`locking`).
    lock_warning: Option<String>,
    /// What the config file's read found wrong, held back while a prompt
    /// at open asks (`say_config`).
    config_warning: Option<String>,
    /// The grid picker `:table` opens, while it shows (`table_keys`).
    picker: Option<TablePicker>,
    /// What the watch last found wrong with the file on disk (`disk`):
    /// never `Back`.
    disk: Option<DiskEvent>,
    /// The watch on the file, once the shell asked for it and there is a
    /// file to watch (`disk`).
    watcher: Option<Watcher>,
    /// The shell asked for the watch (`start_watching`); a file missing
    /// until now is watched from its first write.
    watch_wanted: bool,
    /// When autosave writes the change waiting (`autosave`; the
    /// `marks::now_ns` clock): `None` while nothing is waiting.
    autosave_due: Option<u64>,
    /// This window's recovery copy of the open file, while there is a
    /// state directory to keep it in (`recovering`).
    recovery: Option<Recovery>,
    /// A copy a window that is gone left, newer than the file, while the
    /// recovery prompt is to ask or asks about it (`recovering`).
    recovery_found: Option<Found>,
    /// What vim-off editing keeps: the typing run and the find bar
    /// (`nonvim_keys`).
    plain: NonVim,
    /// The mouse button held since a `Down` or `DoubleClick` the editor
    /// took, until its `Up` (`mouse`).
    pressed: Option<mouse::Pressed>,
    /// The part of a point the wheel has scrolled but the view has not
    /// (`mouse`).
    wheel_rem: f32,
}

impl Editor {
    /// Reads `req.path` and opens it. A missing file opens empty under its
    /// name. Bench mode (`<P>BENCH` set) opens in Insert at the first bold
    /// on a paragraph line. The named registers are shared through the
    /// state directory the environment names; with none, they last only
    /// as long as this window. The file's lock record goes there too
    /// (`open_with_state`).
    pub fn open(req: OpenRequest, cfg: Config, platform: Platform, host: Box<dyn Host>) -> Editor {
        let state = StateDir::from_env();
        Editor::open_with_state(req, cfg, platform, host, state.as_ref())
    }

    /// `open`, sharing the named registers through `state` (with `None`,
    /// keeping them in memory) and taking the file's lock record there
    /// (`locking`). A file read (or found missing) takes the lock: another
    /// live window holding it asks the already-open prompt before any
    /// editing, and with no `state`, or one that cannot be written, the
    /// file opens as usual and says so once. A file that could not be read
    /// takes none.
    pub fn open_with_state(
        req: OpenRequest,
        cfg: Config,
        platform: Platform,
        host: Box<dyn Host>,
        state: Option<&StateDir>,
    ) -> Editor {
        let loaded = file::load(&req.path);
        marks::mark(marks::names::FILE_READ);
        let saver = FileSaver::new(req.path.clone(), &loaded);
        let not_utf8 = matches!(loaded, Loaded::NotUtf8 { .. });
        let lockable = !matches!(loaded, Loaded::Error(_)) && !req.path.as_os_str().is_empty();
        // A file that is missing opens empty under its name; one that is
        // not valid UTF-8 opens empty and read-only (it is never written
        // back); one that cannot be read opens empty, saying so.
        let (buf, message) = match loaded {
            Loaded::Text { buffer, .. } => (buffer, None),
            Loaded::Missing | Loaded::NotUtf8 { .. } => (Buffer::from_text(""), None),
            Loaded::Error(_) => (Buffer::from_text(""), Some(strings::READ_FAILED)),
        };
        let mut editor =
            Editor::build_from(req, buf, message, cfg, platform, host, bench_enabled());
        editor.set_saver(Box::new(saver));
        editor.read_only(not_utf8);
        if let Some(state) = state {
            editor.share_registers(state);
        }
        if lockable {
            // The scan first, so a lock warning can wait for its prompt.
            if let Some(state) = state {
                editor.start_recovery(state);
            }
            editor.lock_at_open(state);
            // After the already-open prompt, if that asks.
            if editor.asking.is_none() {
                editor.offer_recovery();
            }
        }
        marks::mark(marks::names::STATE_SCANNED);
        marks::mark(marks::names::PARSED);
        editor
    }

    /// An editor on `text`, with no file behind it (the tests' and the
    /// `editor_keys` fuzz harness's editors).
    #[cfg(any(test, feature = "fuzzing"))]
    pub(crate) fn build(
        req: OpenRequest,
        text: &str,
        message: Option<&'static str>,
        cfg: Config,
        platform: Platform,
        host: Box<dyn Host>,
        bench: bool,
    ) -> Editor {
        let buf = Buffer::from_text(text);
        Editor::build_from(req, buf, message, cfg, platform, host, bench)
    }

    /// An editor on `buf`, with no file behind it until `set_saver`.
    /// `message` shows from the start and fades like any other (the
    /// shell arms the timer from `open_effects`).
    fn build_from(
        req: OpenRequest,
        buf: Buffer,
        message: Option<&'static str>,
        cfg: Config,
        platform: Platform,
        host: Box<dyn Host>,
        bench: bool,
    ) -> Editor {
        let blocks = Blocks::new(&buf);
        let title = req.display_name.clone().unwrap_or_else(|| {
            req.path.file_name().map_or_else(
                || strings::UNTITLED.to_owned(),
                |n| n.to_string_lossy().into_owned(),
            )
        });
        let cursor = if bench {
            first_bold(&buf, &blocks).unwrap_or_default()
        } else {
            Pos::default()
        };
        // Light until the shell reports the system's appearance.
        let theme = cfg.theme.fixed().unwrap_or(Theme::Light);
        Editor {
            buf,
            blocks,
            tokens: cfg.tokens(theme).0,
            theme,
            cursor,
            goal: None,
            // Bench mode types straight away, so it starts in Insert.
            mode: if cfg.vim && !bench {
                Mode::Normal
            } else {
                Mode::Insert
            },
            anchor: None,
            cmdline: String::new(),
            last_visual: None,
            confirm: None,
            searching: None,
            search: None,
            vim: Vim::new(),
            registers: Registers::new(),
            register: None,
            history: History::new(),
            dirty: false,
            reader: req.view,
            reader_return: None,
            reader_g: false,
            help: None,
            title,
            message: message.map(|m| (m.to_owned(), MsgKind::Warning)),
            message_until: message.map(|_| host.now_ns().saturating_add(status::MESSAGE_FOR_NS)),
            warnings: Vec::new(),
            cfg,
            platform,
            host,
            saver: Box::new(Unwired),
            preedit: None,
            asking: None,
            path: req.path,
            linking: None,
            opening: None,
            lock: None,
            read_only_chosen: false,
            lock_warning: None,
            config_warning: None,
            picker: None,
            disk: None,
            watcher: None,
            watch_wanted: false,
            autosave_due: None,
            recovery: None,
            recovery_found: None,
            plain: NonVim::default(),
            pressed: None,
            wheel_rem: 0.0,
        }
    }

    /// What the shell does once the window is up: arm the timer for a
    /// message the open left, so it fades like any other.
    pub fn open_effects(&self) -> Effects {
        Effects {
            wake_at: self.message_until,
            ..Effects::default()
        }
    }

    /// Marks the document read-only because its file is not valid UTF-8,
    /// or clears that, with the warning beside the file name. The `Saver`
    /// is what refuses the writes.
    fn read_only(&mut self, on: bool) {
        self.warnings.retain(|w| *w != strings::NOT_UTF8_READ_ONLY);
        if on {
            self.warnings.push(strings::NOT_UTF8_READ_ONLY);
        }
    }

    /// Puts the file behind `:w`, `:e!` and closing.
    pub fn set_saver(&mut self, saver: Box<dyn Saver>) {
        self.saver = saver;
    }

    pub fn handle(&mut self, input: Input) -> Effects {
        // With vim off, any input but a tick, a composition, the wheel or
        // the pointer's hover ends the typing run unless it types more of
        // it (`nonvim_keys`).
        let passive = matches!(
            input,
            Input::Tick { .. }
                | Input::Preedit { .. }
                | Input::Scroll { .. }
                | Input::Mouse(MouseEvent {
                    kind: MouseKind::Move,
                    ..
                })
        );
        if !self.cfg.vim && !passive {
            self.plain.input();
        }
        let dirty = self.dirty;
        let autosave = self.autosave_before();
        let status = self.status();
        let current = self.current_match().map(|(line, _)| line);
        let until = self.message_until;
        let line = self.cursor.line;
        let edits = self.buf.shift_mark();
        let mut fx = self.dispatch(input);
        // A reload the file's change waits for, once the input left the
        // document clean and in a mode that can take it (`disk`); never
        // after the input closed the window. A clean `:w` refused as
        // changed on disk ends here too: nothing was lost, the reload
        // takes the file and its message replaces the refusal.
        if fx.close.is_none() {
            fx = fx.merge(self.take_waiting_reload());
        }
        fx = fx.merge(self.drop_stale_preedit(line));
        if fx.close.is_none() {
            fx = fx.merge(self.autosave_after(&autosave));
            // After autosave, which may have left the document clean.
            fx = fx.merge(self.recovery_after(edits));
        }
        // The dirty state is reported here, once per change of it, so an
        // input that passes through several states (an undo to the save
        // point that lets a waiting reload in) reports only where it
        // ended, and one that ends where it began (a reload of a clean
        // document) reports nothing.
        fx.dirty = (self.dirty != dirty).then_some(self.dirty);
        let warnings = [
            (self.registers.take_reset(), strings::NAMED_REGISTERS_RESET),
            (
                self.registers.take_unshared(),
                strings::NAMED_REGISTER_NOT_SHARED,
            ),
        ];
        for (_, text) in warnings.into_iter().filter(|(due, _)| *due) {
            self.say_as(text.to_owned(), MsgKind::Warning, status::MESSAGE_FOR_NS);
        }
        if fx.redraw == Redraw::None && self.status() != status {
            fx.redraw = Redraw::Status;
        }
        let now = self.current_match().map(|(line, _)| line);
        if now != current {
            let n = self.buf.line_count();
            for line in [current, now].into_iter().flatten().filter(|&l| l < n) {
                fx.redraw = std::mem::take(&mut fx.redraw).merge(Redraw::Lines(line..line + 1));
            }
        }
        if self.message_until != until {
            fx = fx.merge(Effects {
                wake_at: self.message_until,
                ..Effects::default()
            });
        }
        fx
    }

    fn dispatch(&mut self, input: Input) -> Effects {
        // Typing while a mouse button is held: the reveal the press held
        // back catches up first (`mouse`).
        if matches!(
            input,
            Input::Key(_) | Input::Text(_) | Input::Preedit { .. }
        ) && self.pressed.is_some()
        {
            let fx = self.release_press();
            return fx.merge(self.dispatch(input));
        }
        match input {
            Input::Key(k) => self.key(k),
            // A commit ends the composition, wherever it lands.
            Input::Text(text) => {
                let fx = self.clear_preedit();
                fx.merge(self.text_input(&ime::normalise_newlines(text)))
            }
            Input::Preedit { text, cursor } => self.set_preedit(text, cursor),
            // With unsaved changes, ask (`prompt`).
            Input::CloseRequested if self.dirty => self.ask(Ask::Close),
            Input::CloseRequested => self.close(0),
            // The recovery copy is written first (`recovering`).
            Input::Signal(s) => self.close_on_signal(s),
            // Clicks, drags, the table picker's hover and the wheel
            // (`mouse`).
            Input::Mouse(m) => self.mouse(m),
            Input::Scroll { dy_pt } => self.wheel(dy_pt),
            // Coming to the front looks at the file at once (`disk`).
            Input::Focus(true) => self.focus_in(),
            // The system's light or dark (`appearance`).
            Input::Appearance(theme) => self.appearance(theme),
            Input::Focus(_) => Effects::default(),
            Input::Tick { now_ns } => self.tick(now_ns),
        }
    }

    /// Committed text types only in Insert. In Normal and Visual it is a
    /// run of keys, so an input method never types into the buffer there.
    fn text_input(&mut self, text: &str) -> Effects {
        if self.mode == Mode::Prompt {
            return self.prompt_text(text);
        }
        // The help overlay takes each char as a key.
        if self.help.is_some() {
            return text.chars().fold(Effects::default(), |fx, c| {
                fx.merge(self.help_key(KeyEvent {
                    key: Key::Char(c),
                    mods: Mods::default(),
                    time_ns: 0,
                }))
            });
        }
        // So does the table picker.
        if self.picker.is_some() {
            return text.chars().fold(Effects::default(), |fx, c| {
                fx.merge(self.picker_key(KeyEvent {
                    key: Key::Char(c),
                    mods: Mods::default(),
                    time_ns: 0,
                }))
            });
        }
        if self.linking() {
            return self.cmdline_input(text);
        }
        // The find bar takes it (vim off).
        if self.finding() {
            return self.find_input(text);
        }
        if !self.cfg.vim {
            return self.plain_type(text);
        }
        if self.mode == Mode::Insert {
            return self.insert(text);
        }
        if self.takes_keys() {
            return self.normal_text(text);
        }
        match self.mode {
            Mode::Search => self.search_input(text),
            Mode::Command => self.cmdline_input(text),
            // In Confirm, each char is an answer.
            Mode::Confirm => text.chars().fold(Effects::default(), |fx, c| {
                fx.merge(self.confirm_key(KeyEvent {
                    key: Key::Char(c),
                    mods: Mods::default(),
                    time_ns: 0,
                }))
            }),
            _ => Effects::default(),
        }
    }

    /// The window title: the display name, or the file name.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The status bar's message, if there is one.
    pub fn message(&self) -> Option<&str> {
        self.message.as_ref().map(|(text, _)| text.as_str())
    }

    /// Shows `text` in the status bar until it fades, as an Error when it
    /// is in vim's error form (`MsgKind::of`).
    fn say(&mut self, text: String) {
        let kind = MsgKind::of(&text);
        self.say_as(text, kind, status::MESSAGE_FOR_NS);
    }

    /// Shows `text` as `kind` for `for_ns` nanoseconds.
    fn say_as(&mut self, text: String, kind: MsgKind, for_ns: u64) {
        self.message = Some((text, kind));
        self.message_until = Some(self.host.now_ns().saturating_add(for_ns));
    }

    /// Clears the status bar's message.
    fn unsay(&mut self) {
        self.message = None;
        self.message_until = None;
    }

    /// The main loop woke: what the file watch found is taken (`disk`),
    /// a write autosave has due goes (`autosave`), so does a recovery copy
    /// due (`recovering`), then a timed message whose time is up goes. One
    /// not due yet (a timer that fired early) asks to be woken again then.
    fn tick(&mut self, now_ns: u64) -> Effects {
        let fx = self.poll_disk();
        let fx = fx.merge(self.autosave_tick(now_ns));
        let fx = fx.merge(self.recovery_tick(now_ns));
        fx.merge(self.fade_message(now_ns))
    }

    /// Clears the message if its time is up at `now_ns`.
    fn fade_message(&mut self, now_ns: u64) -> Effects {
        match self.message_until {
            None => return Effects::default(),
            Some(until) if now_ns < until => {
                return Effects {
                    wake_at: Some(until),
                    ..Effects::default()
                };
            }
            Some(_) => {}
        }
        self.unsay();
        Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The search being typed, or else the last one confirmed.
    pub fn search(&self) -> Option<&Search> {
        match &self.searching {
            Some(s) => Some(&s.search),
            None => self.search.as_ref(),
        }
    }

    /// The pending count and keys, for the status line (Task 26).
    pub fn showcmd(&self) -> String {
        self.vim.showcmd()
    }

    /// What register `name` holds (`None` is the unnamed register).
    pub fn register(&mut self, name: Option<char>) -> Option<Register> {
        self.registers.get(name)
    }

    /// Shares the named registers with every other window through the
    /// store in `state`.
    pub fn share_registers(&mut self, state: &StateDir) {
        self.registers.share(RegisterStore::new(state));
    }

    /// The command line's text, as typed after `:` (it starts as `'<,'>`
    /// when `:` is pressed in Visual).
    pub fn cmdline(&self) -> &str {
        &self.cmdline
    }

    /// The question Prompt or Confirm mode asks, while it asks it.
    pub fn prompt(&self) -> Option<String> {
        if let Some(p) = &self.asking {
            return Some(p.question(self.opening.as_deref()));
        }
        self.confirm.as_ref().map(ConfirmSession::prompt)
    }

    /// The selection Visual had when `:` left it, for the `'<,'>` range.
    pub fn last_visual(&self) -> Option<Selection> {
        self.last_visual
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buf
    }

    /// The undo history.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// The change `.` repeats.
    pub fn last_change(&self) -> Option<&Change> {
        self.vim.last_change()
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// The whole document, as it would be saved.
    pub fn text(&self) -> String {
        self.buf.to_text()
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn host(&mut self) -> &mut dyn Host {
        self.host.as_mut()
    }

    fn key(&mut self, k: KeyEvent) -> Effects {
        // A prompt takes every key, with or without vim.
        if self.mode == Mode::Prompt {
            return self.prompt_key(k);
        }
        // So does the help overlay.
        if self.help.is_some() {
            return self.help_key(k);
        }
        // And the table picker (`table_keys`).
        if self.picker.is_some() {
            return self.picker_key(k);
        }
        // And the find bar, with vim off (`nonvim_keys`).
        if self.finding() {
            return self.find_key(k);
        }
        let ctrl_c = self.cfg.vim
            && k.key == Key::Char('c')
            && k.mods
                == Mods {
                    ctrl: true,
                    ..Mods::default()
                };
        let primary = self.platform.primary(k.mods);
        // The clipboard keys. Ctrl+C copies a Visual selection too; with
        // none it is Esc, as in vim.
        match k.key {
            Key::Char('c') if (primary || ctrl_c) && self.visual() => return self.copy(),
            // With vim off these go with the other chords, below.
            Key::Char('x') if primary && self.cfg.vim => return self.cut(),
            Key::Char('v') if primary && (self.cfg.vim || self.linking()) => {
                return self.paste();
            }
            // Primary+Shift+R turns reader view on and off.
            Key::Char('r' | 'R') if primary && k.mods.shift && !k.mods.alt => {
                return self.toggle_reader();
            }
            // Primary+L opens the link prompt (`link`).
            Key::Char('l') if primary && !k.mods.shift && !k.mods.alt => {
                return self.start_link();
            }
            // Primary+S is `:w`, from any mode but Confirm, whose `:s///c`
            // session is one undo step a save point must not split. Reader
            // view is read-only, so it does nothing there (nor can `:w`
            // run, with `:` disabled).
            Key::Char('s') if primary && !k.mods.shift && !k.mods.alt => {
                if self.mode == Mode::Confirm || self.reader {
                    return Effects::default();
                }
                // With vim off, a file changed on disk asks.
                if !self.cfg.vim {
                    return self.plain_save().merge(Effects {
                        redraw: Redraw::Status,
                        ..Effects::default()
                    });
                }
                return self.ex(ExCmd::Write { force: false }).merge(Effects {
                    redraw: Redraw::Status,
                    ..Effects::default()
                });
            }
            _ => {}
        }
        let ctrl_only = Mods {
            ctrl: true,
            ..Mods::default()
        };
        if k.key == Key::Space && k.mods == ctrl_only {
            return self.ctrl_space(k);
        }
        let k = if ctrl_c {
            KeyEvent {
                key: Key::Esc,
                mods: Mods::default(),
                ..k
            }
        } else {
            k
        };
        // Esc clears a message, and goes on to do what it does.
        if k.key == Key::Esc {
            self.unsay();
        }
        // With vim off, every other key is the non-vim engine's
        // (`nonvim_keys`), but on the link prompt and in reader view.
        if !self.cfg.vim && !self.reader && !self.linking() {
            return self.plain_key(k);
        }
        let redo = self.cfg.vim
            && self.mode == Mode::Normal
            && k.key == Key::Char('r')
            && k.mods
                == Mods {
                    ctrl: true,
                    ..Mods::default()
                };
        // The modifiers after Ctrl+C became Esc.
        if (k.mods.ctrl || self.platform.primary(k.mods)) && !redo {
            // Shortcuts arrive with later tasks.
            return Effects::default();
        }
        // The link prompt is the command line, with vim or without.
        if self.linking() {
            return self.command_key(k);
        }
        // Reader view's keys scroll the view (`reader`).
        if self.reader
            && matches!(self.mode, Mode::Normal | Mode::Insert)
            && let Some(fx) = self.reader_key(k)
        {
            return fx;
        }
        if self.cfg.vim {
            match self.mode {
                Mode::Normal | Mode::Visual | Mode::VisualLine => return self.normal_key(k),
                Mode::Insert if k.key == Key::Esc => return self.leave_insert(),
                Mode::Insert => {}
                Mode::Command => return self.command_key(k),
                Mode::Search => return self.search_key(k),
                Mode::Confirm => return self.confirm_key(k),
                // Taken at the top.
                Mode::Prompt => return Effects::default(),
            }
        }
        let arrow = matches!(
            k.key,
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End
        );
        if self.cfg.vim && arrow {
            // Vim: moving in Insert ends the undo step and the change `.`
            // repeats, and starts new ones.
            self.history.end();
            self.history.begin();
            self.vim.arrowed();
        }
        let line = self.buf.line(self.cursor.line);
        let col = self.buf.clamp(self.cursor).col;
        match k.key {
            Key::Char(c) => self.insert(c.encode_utf8(&mut [0; 4])),
            Key::Space => self.insert(" "),
            Key::Enter => self.insert_list_key(ListKey::Enter),
            Key::Tab if k.mods.shift => self.insert_list_key(ListKey::BackTab),
            Key::Tab => self.insert_list_key(ListKey::Tab),
            Key::Backspace => self.backspace(),
            Key::Delete => self.delete(),
            Key::Left => {
                let to = if col > 0 {
                    self.at(grapheme::prev(line, col))
                } else if self.cursor.line > 0 {
                    self.line_end(self.cursor.line - 1)
                } else {
                    return Effects::default();
                };
                self.move_across(to)
            }
            Key::Right => {
                let to = if col < line.len() {
                    self.at(grapheme::next(line, col))
                } else if self.cursor.line + 1 < self.buf.line_count() {
                    Pos {
                        line: self.cursor.line + 1,
                        col: 0,
                    }
                } else {
                    return Effects::default();
                };
                self.move_across(to)
            }
            Key::Home => self.move_across(self.at(0)),
            Key::End => self.move_across(self.line_end(self.cursor.line)),
            Key::Up => self.vertical(false),
            Key::Down => self.vertical(true),
            // TODO(later tasks): paging. Esc only means something in vim,
            // which handles it above.
            Key::Esc | Key::PageUp | Key::PageDown => Effects::default(),
        }
    }

    /// Carries out what the vim engine makes of a Normal-mode key, as one
    /// undo step. A key that enters Insert leaves the step open until Esc,
    /// and the change it starts waits for Esc to become the one `.`
    /// repeats.
    ///
    /// Visual keys come here too: the selection's lines, before and after,
    /// are repainted.
    fn normal_key(&mut self, k: KeyEvent) -> Effects {
        let before = self.selection_lines();
        self.history.begin();
        let fx = self.normal_step(k);
        if self.mode != Mode::Insert {
            self.history.end();
        }
        if self.refuses_edits() {
            self.vim.discard();
        } else if self.mode == Mode::Normal {
            self.vim.commit();
        }
        fx.merge(self.selection_redraw(before))
    }

    /// The cursor with the column vertical motions aim for.
    fn vim_cursor(&self) -> Cursor {
        Cursor {
            pos: self.cursor,
            want: self
                .goal
                .unwrap_or_else(|| Want::Col(self.buf.display_col(self.cursor))),
        }
    }

    fn normal_step(&mut self, k: KeyEvent) -> Effects {
        // `t` in a table starts a table command (`table_keys`), which
        // only the editor, holding the blocks, can tell. Where edits are
        // refused it is no prefix (reader view's keys go on scrolling).
        let plain_t = k.key == Key::Char('t') && k.mods == Mods::default();
        if plain_t
            && self.mode == Mode::Normal
            && !self.refuses_edits()
            && crate::tables::table_at(&self.blocks, self.cursor.line).is_some()
            && self.vim.table_prefix()
        {
            return Effects {
                redraw: Redraw::Status,
                ..Effects::default()
            };
        }
        let cur = self.vim_cursor();
        let step = match self.shown_selection().filter(|_| self.visual()) {
            Some(sel) => self.vim.visual_key(&k, &self.buf, cur, sel),
            None => self.vim.key(&k, &self.buf, cur),
        };
        if step != Step::Pending {
            self.register = self.vim.take_register();
        }
        let fx = self.carry_out(step);
        self.register = None;
        fx
    }

    /// Does what a vim step asks, with `self.register` naming the register
    /// it uses.
    fn carry_out(&mut self, step: Step) -> Effects {
        let status = Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        };
        match step {
            Step::Pending => status,
            // Reader view never writes: with changes, `ZZ` refuses and
            // says how to leave without them, as vim does read-only.
            Step::Quit { write: true } if self.reader && self.dirty => {
                self.say_as(
                    strings::READ_ONLY_ZQ.to_owned(),
                    MsgKind::Error,
                    status::MESSAGE_FOR_NS,
                );
                status
            }
            Step::Quit { write } => {
                let cmd = if write {
                    ExCmd::Exit
                } else {
                    ExCmd::Quit { force: true }
                };
                status.merge(self.ex(cmd))
            }
            Step::Visual { linewise } => {
                if !self.visual() {
                    self.anchor = Some(self.cursor);
                }
                self.mode = if linewise {
                    Mode::VisualLine
                } else {
                    Mode::Visual
                };
                status
            }
            Step::Leave => {
                self.leave_visual();
                status
            }
            // Reader view disables the command line.
            Step::Cmdline if self.reader => status,
            Step::Cmdline => {
                // From Visual, the selection is remembered for `'<,'>`
                // (it stays remembered, as vim's marks do).
                self.cmdline = match self.shown_selection() {
                    Some(sel) => {
                        self.last_visual = Some(sel);
                        "'<,'>".to_owned()
                    }
                    None => String::new(),
                };
                self.leave_visual();
                self.unsay();
                self.mode = Mode::Command;
                status
            }
            // Reader view refuses searching too (R57): its line would be
            // typed in the hidden status bar.
            Step::Search { .. } | Step::SearchNext { .. } if self.reader => status,
            Step::Search { forward, op, count } => self.start_search(forward, op, count),
            Step::SearchNext { reverse, op, count } => {
                status.merge(self.search_next(reverse, op, count))
            }
            Step::InnerCode { op } => self.inner_code(op),
            Step::Move(c) => {
                let fx = self.move_to(c.pos);
                self.goal = Some(c.want);
                fx
            }
            Step::Insert(_) | Step::Open { .. } if self.refuses_edits() => Effects::default(),
            Step::Insert(p) => {
                self.mode = Mode::Insert;
                self.move_across(p)
            }
            Step::Open { above } => self.run(|ctx| operator::open_line(ctx, above)),
            Step::Operate { op, range } => {
                self.leave_visual();
                self.operate(op, range)
            }
            Step::Put { before, count } => self.put(before, count),
            Step::Undo { count } => self.undo(false, count),
            Step::Redo { count } => self.undo(true, count),
            Step::Repeat { count } => {
                self.leave_visual();
                self.repeat(count)
            }
            Step::List(key) => self.normal_list_key(key),
            Step::Table(key) => self.table_command(key),
        }
    }

    /// The selection, while Visual is up or a search from Visual is.
    fn shown_selection(&self) -> Option<Selection> {
        // Vim off: the selection Shift and the arrows make
        // (`nonvim_keys`).
        if !self.cfg.vim {
            return self.plain_selection().filter(|_| self.mode == Mode::Insert);
        }
        let linewise = match self.mode {
            Mode::Visual => false,
            Mode::VisualLine => true,
            Mode::Search => match self.searching.as_ref()?.ret {
                Mode::Visual => false,
                Mode::VisualLine => true,
                _ => return None,
            },
            _ => return None,
        };
        self.anchor.map(|anchor| Selection {
            anchor,
            head: self.cursor,
            linewise,
        })
    }

    /// True in Visual or Visual Line.
    fn visual(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
    }

    /// True in the modes where a key, or committed text, is a vim command.
    fn takes_keys(&self) -> bool {
        self.mode == Mode::Normal || self.visual()
    }

    /// Back to Normal from Visual, with the cursor where it is. Outside
    /// Visual it does nothing.
    fn leave_visual(&mut self) {
        if self.visual() {
            self.mode = Mode::Normal;
            self.anchor = None;
            self.cursor = vim::rest(&self.buf, self.cursor);
        }
    }

    /// The lines the selection covers, if one shows.
    fn selection_lines(&self) -> Option<Range<usize>> {
        let (a, b) = self.shown_selection()?.ordered();
        Some(a.line..b.line + 1)
    }

    /// Repaints the lines the selection covered (`before`, clamped to the
    /// buffer now) and covers now.
    fn selection_redraw(&self, before: Option<Range<usize>>) -> Effects {
        let n = self.buf.line_count();
        let redraw = [before, self.selection_lines()]
            .into_iter()
            .flatten()
            .map(|r| r.start.min(n)..r.end.min(n))
            .filter(|r| !r.is_empty())
            .fold(Redraw::None, |acc, r| acc.merge(Redraw::Lines(r)));
        Effects {
            redraw,
            ..Effects::default()
        }
    }

    /// `/` or `?`: Search mode, from the cursor as it is now. The last
    /// search's highlights give way to the new one's.
    fn start_search(&mut self, forward: bool, op: Option<Operator>, count: usize) -> Effects {
        let origin = self.vim_cursor();
        self.searching = Some(Searching {
            search: Search::new(forward, origin),
            ret: self.mode,
            op,
            count,
            goal: self.goal,
            register: self.register.filter(|_| op.is_some()),
        });
        self.mode = Mode::Search;
        self.unsay();
        Effects {
            redraw: if self.search.is_some() {
                Redraw::All
            } else {
                Redraw::Status
            },
            ..Effects::default()
        }
    }

    /// A key on the search line. Typing and Backspace search again from
    /// the start; Backspace on an empty query leaves, as Esc does. Enter
    /// confirms. Other keys do nothing.
    fn search_key(&mut self, k: KeyEvent) -> Effects {
        match k.key {
            Key::Esc => self.end_search(false),
            Key::Enter => self.end_search(true),
            Key::Backspace => {
                let Some(s) = self.searching.as_mut() else {
                    return Effects::default();
                };
                let query = &mut s.search.query;
                if query.is_empty() {
                    return self.end_search(false);
                }
                query.truncate(grapheme::prev(query, query.len()));
                self.search_again()
            }
            Key::Char(c) => self.search_input(c.encode_utf8(&mut [0; 4])),
            Key::Space => self.search_input(" "),
            _ => Effects::default(),
        }
    }

    /// Text typed on the search line. A line break cannot be in a match,
    /// so it is dropped.
    fn search_input(&mut self, text: &str) -> Effects {
        let Some(s) = self.searching.as_mut() else {
            return Effects::default();
        };
        s.search
            .query
            .extend(text.chars().filter(|c| !matches!(c, '\n' | '\r')));
        self.search_again()
    }

    /// Searches for the query as it is now from where the search started,
    /// moving the cursor to the match (or back to the start when there is
    /// none). The highlights change everywhere, so all is repainted.
    fn search_again(&mut self) -> Effects {
        let Some(s) = self.searching.as_mut() else {
            return Effects::default();
        };
        s.search.refresh(&self.buf);
        let origin = s.search.origin.pos;
        let hit = search::find(
            &self.buf,
            &s.search.query,
            origin,
            s.search.forward,
            s.count,
        );
        s.search.set_current(hit.as_ref());
        self.cursor = hit.map_or(origin, |h| h.pos());
        Effects {
            redraw: Redraw::All,
            scroll_to_cursor: true,
            ..Effects::default()
        }
    }

    /// Esc (`confirm` false) or Enter on the search line: back to the mode
    /// the search came from. Esc puts the cursor back and keeps the last
    /// search. Enter makes this one the last (an empty query repeats the
    /// last one's), then goes to its match, or applies the waiting
    /// operator up to it, or says there is none.
    fn end_search(&mut self, confirm: bool) -> Effects {
        let before = self.selection_lines();
        let Some(Searching {
            mut search,
            ret,
            op,
            count,
            goal,
            register,
        }) = self.searching.take()
        else {
            return Effects::default();
        };
        self.mode = ret;
        if !self.visual() {
            self.anchor = None;
        }
        let origin = search.origin.pos;
        self.cursor = origin;
        if confirm
            && search.query.is_empty()
            && let Some(last) = &self.search
        {
            search.query.clone_from(&last.query);
        }
        let all = Effects {
            redraw: Redraw::All,
            scroll_to_cursor: true,
            ..Effects::default()
        };
        let hit = confirm
            .then(|| search::find(&self.buf, &search.query, origin, search.forward, count))
            .flatten();
        let found = match hit {
            Some(hit) => hit,
            None => {
                self.goal = goal;
                if op.is_some() {
                    self.vim.discard();
                }
                if confirm && !search.query.is_empty() {
                    // vim: the pattern is the last search all the same.
                    self.say(strings::pattern_not_found(&search.query));
                    search.refresh(&self.buf);
                    self.search = Some(search);
                }
                return all.merge(self.selection_redraw(before));
            }
        };
        if op.is_some() {
            self.vim.searched(&search.query, search.forward);
        }
        search.refresh(&self.buf);
        search.set_current(Some(&found));
        self.search = Some(search);
        // Like a Normal-mode command: one undo step, and the change `.`
        // repeats once it is done.
        self.history.begin();
        self.register = register;
        let fx = self.go_to_match(op, &found);
        self.register = None;
        if self.mode != Mode::Insert {
            self.history.end();
        }
        if self.refuses_edits() {
            self.vim.discard();
        } else if self.mode == Mode::Normal {
            self.vim.commit();
        }
        all.merge(fx).merge(self.selection_redraw(before))
    }

    /// `n` (or, `reverse`, `N`): the `count`th match of the last search
    /// from the cursor. With no search, or no match, the operator waiting
    /// for it does nothing.
    fn search_next(&mut self, reverse: bool, op: Option<Operator>, count: usize) -> Effects {
        let Some(last) = self.search.as_mut() else {
            self.vim.discard();
            return Effects::default();
        };
        let forward = last.forward != reverse;
        let hit = search::find(&self.buf, &last.query, self.cursor, forward, count);
        last.set_current(hit.as_ref());
        match hit {
            Some(hit) => self.go_to_match(op, &hit),
            None => {
                let text = strings::pattern_not_found(&last.query);
                self.vim.discard();
                self.say(text);
                Effects::default()
            }
        }
    }

    /// Moves the cursor to `hit`, or applies `op` from the cursor up to it
    /// (exclusive, charwise, as vim does), and says when the search
    /// wrapped. An operator with nothing between does nothing.
    fn go_to_match(&mut self, op: Option<Operator>, hit: &Hit) -> Effects {
        if hit.wrapped {
            self.say_as(
                strings::SEARCH_WRAPPED.to_owned(),
                MsgKind::Info,
                search::WRAPPED_FOR_NS,
            );
        } else {
            self.unsay();
        }
        let status = Effects {
            redraw: Redraw::Status,
            ..Effects::default()
        };
        let fx = match op {
            None => self.move_across(hit.pos()),
            Some(op) => match OpRange::exclusive(&self.buf, self.cursor, hit.pos()) {
                Some(range) => self.operate(op, range),
                None => {
                    self.vim.discard();
                    Effects::default()
                }
            },
        };
        status.merge(fx)
    }

    /// `yic dic cic`: the operator over the inner code block at the cursor.
    /// Outside a block, or on an empty one (but for `cic`), nothing.
    fn inner_code(&mut self, op: Operator) -> Effects {
        let block = textobj::code_block(&self.blocks, self.cursor.line);
        let block = match block {
            Some(b)
                if textobj::acts(op, Some(&b))
                    && (!self.refuses_edits() || op == Operator::Yank) =>
            {
                b
            }
            _ => {
                self.vim.discard();
                return Effects {
                    redraw: Redraw::Status,
                    ..Effects::default()
                };
            }
        };
        self.run(|ctx| textobj::operate(ctx, op, block))
    }

    /// Primary+C, and Ctrl+C, in Visual: copies the selection's raw text
    /// to the clipboard (and the unnamed register), staying in Visual.
    fn copy(&mut self) -> Effects {
        let Some(sel) = self.shown_selection() else {
            return Effects::default();
        };
        let reg = visual::register(&self.buf, sel);
        if !reg.text.is_empty() {
            operator::write_register(self.host.as_mut(), &mut self.registers, reg);
        }
        Effects::default()
    }

    /// Primary+X in Visual: copies the selection, then deletes it, as `d`
    /// does. Without a selection, or in reader mode, nothing.
    fn cut(&mut self) -> Effects {
        if !self.visual() || self.refuses_edits() {
            return Effects::default();
        }
        self.vim.cancel();
        self.normal_key(KeyEvent {
            key: Key::Char('d'),
            mods: Mods::default(),
            time_ns: 0,
        })
    }

    /// Primary+V: puts the clipboard's text in literally (no list
    /// continuation, no linewise put) as one undo step. In Visual it
    /// replaces the selection, leaving the clipboard as it is, and returns
    /// to Normal. In Insert the cursor ends after the text; in Normal on
    /// its last grapheme.
    fn paste(&mut self) -> Effects {
        // The link prompt takes the clipboard as typed text (a URL to
        // paste in), line breaks dropped.
        if self.linking() {
            let text = self.host.clipboard_get().unwrap_or_default();
            return self.cmdline_input(&text);
        }
        let active = self.takes_keys() || self.mode == Mode::Insert;
        if self.refuses_edits() || !active {
            return Effects::default();
        }
        let Some(mut text) = self
            .host
            .clipboard_get()
            .filter(|t| !t.is_empty())
            .map(|t| t.replace("\r\n", "\n"))
        else {
            return Effects::default();
        };
        self.vim.cancel();
        let sel = self.shown_selection().filter(|_| self.visual());
        let range = match sel {
            Some(sel) => visual::paste_range(&self.buf, sel, &mut text),
            None => {
                let at = self.buf.clamp(self.cursor);
                at..at
            }
        };
        // Before anything changes: a refused paste leaves the text, the
        // mode and an Insert session as they were.
        if self.refuses_paste(range.clone(), &text) {
            return Effects::default();
        }
        let before = self.selection_lines();
        if sel.is_some() {
            self.leave_visual();
        }
        let insert = self.mode == Mode::Insert;
        // In a vim Insert session the paste is a step of its own; the
        // session goes on after it. Its text joins what `.` repeats.
        let session = self.cfg.vim && insert;
        if session {
            self.history.end();
            self.vim.typed(&text);
        }
        let fx = self.run(|ctx| {
            let end = ctx.replace(range, &text).range.end;
            let cursor = if insert {
                end
            } else {
                vim::rest(
                    ctx.buf,
                    Pos {
                        col: grapheme::prev(ctx.buf.line(end.line), end.col),
                        ..end
                    },
                )
            };
            OpOutcome {
                cursor,
                insert: false,
            }
        });
        if session {
            self.history.begin();
        }
        fx.merge(self.selection_redraw(before))
    }

    /// Whether a paste of `text` over `r` is refused, saying so if it is.
    /// A paste is checked as a put is: against the per-put caps, and against
    /// the document budgets for what it adds beyond `r`.
    fn refuses_paste(&mut self, r: Range<Pos>, text: &str) -> bool {
        let breaks = text.matches('\n').count();
        let refused = text.len() > operator::MAX_PUT_BYTES
            || breaks > operator::MAX_PUT_LINES
            || !crate::limits::fits_replacing(&self.buf, r, text);
        if refused {
            self.say(strings::RESULTING_TEXT_TOO_LONG.to_owned());
        }
        refused
    }

    /// Undoes (or, with `redo`, redoes) up to `count` steps and puts the
    /// cursor where the last one says, on a grapheme where Normal mode may
    /// rest; with vim off, where it says unrested on an undo, and after the
    /// text put back on a redo. With nothing to undo it does nothing.
    fn undo(&mut self, redo: bool, count: usize) -> Effects {
        if self.refuses_edits() {
            return Effects::default();
        }
        let mut landed = None;
        for _ in 0..count.max(1) {
            let step = if redo {
                self.history.redo_step(&mut self.buf)
            } else {
                self.history.undo_step(&mut self.buf)
            };
            match step {
                Some(l) => landed = Some(l),
                None => break,
            }
        }
        let Some(landed) = landed else {
            return Effects::default();
        };
        // A step can touch many places, so rebuild and repaint it all.
        self.blocks = Blocks::new(&self.buf);
        if let Some(s) = self.search.as_mut() {
            s.refresh(&self.buf);
        }
        // Vim rests the cursor where Normal mode may. With vim off a
        // cursor may sit at a line end, and a redo goes after the text it
        // put back.
        self.cursor = match (self.cfg.vim, redo) {
            (true, _) => vim::rest(&self.buf, landed.at),
            (false, false) => self.buf.clamp(landed.at),
            (false, true) => self.buf.clamp(landed.end),
        };
        self.goal = None;
        // Landing on the save point is the text on disk again.
        let dirty = !self.history.at_saved();
        let was_dirty = std::mem::replace(&mut self.dirty, dirty);
        Effects {
            redraw: Redraw::All,
            dirty: (was_dirty != dirty).then_some(dirty),
            scroll_to_cursor: true,
            ..Effects::default()
        }
    }

    /// `.`: replays the last change at the cursor.
    fn repeat(&mut self, count: Option<usize>) -> Effects {
        if self.refuses_edits() {
            return Effects::default();
        }
        let last = self.search.as_ref().map(|s| (s.query.clone(), s.forward));
        let last = last
            .as_ref()
            .map(|(query, forward)| (query.as_str(), *forward));
        let mut vim = std::mem::take(&mut self.vim);
        let fx = self.run(|ctx| vim.repeat(ctx, count, last));
        self.vim = vim;
        // vim: repeating `d/foo` searches for `foo` again, so it is the
        // last search (and the highlights) once more.
        if let Some(Change::Search {
            pattern: Some((query, forward)),
            ..
        }) = self.vim.last_change()
        {
            let mut s = Search::new(*forward, self.vim_cursor());
            s.query.clone_from(query);
            s.refresh(&self.buf);
            self.search = Some(s);
            return fx.merge(Effects {
                redraw: Redraw::All,
                ..Effects::default()
            });
        }
        fx
    }

    /// Applies an operator. Reader mode (and a document opened read-only)
    /// only yanks.
    fn operate(&mut self, op: Operator, range: OpRange) -> Effects {
        if self.refuses_edits() && op != Operator::Yank {
            return Effects::default();
        }
        self.run(|ctx| operator::operate(ctx, op, range))
    }

    fn put(&mut self, before: bool, count: usize) -> Effects {
        if self.refuses_edits() {
            return Effects::default();
        }
        self.run(|ctx| operator::put(ctx, before, count))
    }

    /// Committed text in Normal or Visual mode is a run of keys. Once one
    /// of them enters Insert, the rest is typed; once one enters Search or
    /// Command, the rest is the query or the command line.
    fn normal_text(&mut self, text: &str) -> Effects {
        let mut fx = Effects::default();
        for (i, c) in text.char_indices() {
            if self.mode == Mode::Insert {
                return fx.merge(self.insert(&text[i..]));
            }
            if self.mode == Mode::Search {
                return fx.merge(self.search_input(&text[i..]));
            }
            if self.mode == Mode::Command {
                return fx.merge(self.cmdline_input(&text[i..]));
            }
            if !self.takes_keys() {
                return fx;
            }
            let k = KeyEvent {
                key: Key::Char(c),
                mods: Mods::default(),
                time_ns: 0,
            };
            fx = fx.merge(self.normal_key(k));
        }
        fx
    }

    /// Esc in Insert: back to Normal, one grapheme left. The session's
    /// undo step and change end here.
    fn leave_insert(&mut self) -> Effects {
        self.mode = Mode::Normal;
        self.vim.cancel();
        let fx = self.move_across(vim::leave_insert(&self.buf, self.cursor));
        self.history.end();
        self.vim.commit();
        fx
    }

    /// A position on the cursor's line.
    fn at(&self, col: usize) -> Pos {
        Pos {
            line: self.cursor.line,
            col,
        }
    }

    fn line_end(&self, line: usize) -> Pos {
        Pos {
            line,
            col: self.buf.line(line).len(),
        }
    }

    fn insert(&mut self, text: &str) -> Effects {
        let at = self.buf.clamp(self.cursor);
        if !self.refuses_edits() {
            self.vim.typed(text);
        }
        self.edit(at..at, text)
    }

    fn backspace(&mut self) -> Effects {
        let Some(r) = operator::backspace_range(&self.buf, self.cursor) else {
            return Effects::default();
        };
        if !self.refuses_edits() {
            self.vim.backspaced();
        }
        self.edit(r, "")
    }

    fn delete(&mut self) -> Effects {
        let c = self.buf.clamp(self.cursor);
        let line = self.buf.line(c.line);
        let to = if c.col < line.len() {
            Pos {
                line: c.line,
                col: grapheme::next(line, c.col),
            }
        } else if c.line + 1 < self.buf.line_count() {
            Pos {
                line: c.line + 1,
                col: 0,
            }
        } else {
            return Effects::default();
        };
        self.edit(c..to, "")
    }

    /// Replaces `r` (on grapheme boundaries) with `text`, leaves the cursor
    /// after it, and redraws the lines whose text or kind changed plus the
    /// blocks the cursor left and entered.
    fn edit(&mut self, r: Range<Pos>, text: &str) -> Effects {
        if self.refuses_edits() {
            return Effects::default();
        }
        self.run(|ctx| {
            let inverse = ctx.replace(r, text);
            OpOutcome {
                cursor: inverse.range.end,
                insert: false,
            }
        })
    }

    /// Runs `f` over an `EditCtx` on this editor's buffer, then brings the
    /// block structure, cursor, mode and dirty state up to date and redraws
    /// what changed.
    fn run(&mut self, f: impl FnOnce(&mut EditCtx) -> OpOutcome) -> Effects {
        let old = self.reveal_extent(self.cursor.line);
        self.history.begin();
        let mut ctx = EditCtx::new(
            &mut self.buf,
            self.cursor,
            &mut self.history,
            &mut self.registers,
            self.host.as_mut(),
        );
        ctx.register = self.register;
        let out = f(&mut ctx);
        let touched = ctx.touched().to_vec();
        let refusal = ctx.refusal;
        if out.insert {
            self.mode = Mode::Insert;
        }
        let fx = self.finish(old, &touched, out.cursor);
        self.history.end();
        if let Some(text) = refusal {
            self.say(text.to_owned());
        }
        fx
    }

    /// After the edits in `touched`: updates the blocks, puts the cursor at
    /// `cursor` and redraws the changed lines, the reveal extent the cursor
    /// had (`old`, in the buffer before) and the one it has now.
    fn finish(&mut self, old: Range<usize>, touched: &[Touched], cursor: Pos) -> Effects {
        let n = self.buf.line_count();
        let redraw = match touched {
            [] => Redraw::Lines(old),
            [t] => {
                let changed =
                    self.blocks
                        .update(&self.buf, t.start_line..t.new_end_line + 1, t.delta);
                // The old extent, in the new buffer: lines after the
                // replaced range moved by `delta`.
                let shift = |i: usize| {
                    if i > t.old_end_line {
                        i.saturating_add_signed(t.delta)
                    } else {
                        i
                    }
                };
                let old = shift(old.start).min(n)..(shift(old.end - 1) + 1).min(n);
                Redraw::Lines(changed).merge(Redraw::Lines(old))
            }
            // Several edits (`:s` over many lines): rebuild it all.
            _ => {
                self.blocks = Blocks::new(&self.buf);
                Redraw::All
            }
        };
        // The highlights follow the edit, on the lines it touched.
        if let Some(s) = self.search.as_mut() {
            match touched {
                [] => {}
                [t] => s.edited(&self.buf, t),
                _ => s.refresh(&self.buf),
            }
        }
        self.cursor = self.buf.clamp(cursor);
        self.goal = None;
        // Vim off: an edit drops the selection.
        if !self.cfg.vim {
            self.anchor = None;
        }
        let new = self.reveal_extent(self.cursor.line);
        let dirty = if touched.is_empty() {
            None
        } else {
            let was_dirty = std::mem::replace(&mut self.dirty, true);
            (!was_dirty).then_some(true)
        };
        Effects {
            redraw: redraw.merge(Redraw::Lines(new)),
            dirty,
            scroll_to_cursor: true,
            ..Effects::default()
        }
    }

    /// A horizontal motion: forgets the Up/Down goal column.
    fn move_across(&mut self, to: Pos) -> Effects {
        self.goal = None;
        self.move_to(to)
    }

    fn vertical(&mut self, down: bool) -> Effects {
        let line = self.cursor.line;
        let target = if down {
            if line + 1 >= self.buf.line_count() {
                return Effects::default();
            }
            line + 1
        } else {
            let Some(up) = line.checked_sub(1) else {
                return Effects::default();
            };
            up
        };
        let goal = self.goal.unwrap_or_else(|| {
            Want::Col(self.buf.display_col(Pos {
                line,
                col: self.cursor.col,
            }))
        });
        self.goal = Some(goal);
        let text = self.buf.line(target);
        let col = match goal {
            Want::End => text.len(),
            Want::Col(g) => self.buf.grapheme_start(target, g).unwrap_or(text.len()),
        };
        self.move_to(Pos { line: target, col })
    }

    /// Moves the cursor, redrawing what it revealed and what it reveals.
    fn move_to(&mut self, to: Pos) -> Effects {
        let old = self.reveal_extent(self.cursor.line);
        self.cursor = self.buf.clamp(to);
        let new = self.reveal_extent(self.cursor.line);
        Effects {
            redraw: Redraw::Lines(old).merge(Redraw::Lines(new)),
            scroll_to_cursor: true,
            ..Effects::default()
        }
    }

    fn group(&self, line: usize) -> Option<Group> {
        match *self.blocks.kind(line) {
            LineKind::FenceOpen { block, .. }
            | LineKind::FenceClose { block }
            | LineKind::Code { block } => Some(Group::Code(block)),
            LineKind::TableHeader { table }
            | LineKind::TableSep { table }
            | LineKind::TableRow { table } => Some(Group::Table(table)),
            _ => None,
        }
    }

    /// The lines whose display depends on whether the cursor is on `line`:
    /// its whole code block or table, or just the line.
    fn reveal_extent(&self, line: usize) -> Range<usize> {
        let n = self.buf.line_count();
        match self.group(line) {
            None => line..line + 1,
            Some(Group::Table(id)) => {
                let t = self.blocks.table(id);
                t.header..t.last + 1
            }
            Some(Group::Code(id)) => {
                let info = self.blocks.code_block(id);
                let end = match info.close {
                    Some(c) => c + 1,
                    // Unclosed does not mean "to the end": a later opening
                    // fence ends the block too. Its lines are one contiguous
                    // run (ids are never reused), so search for where it stops.
                    None => {
                        let g = Some(Group::Code(id));
                        let (mut lo, mut hi) = (line + 1, n);
                        while lo < hi {
                            let mid = lo + (hi - lo) / 2;
                            if self.group(mid) == g {
                                lo = mid + 1;
                            } else {
                                hi = mid;
                            }
                        }
                        lo
                    }
                };
                info.open..end
            }
        }
    }

    /// What line `i` reveals: around the cursor, or, while a mouse button
    /// is held, around where the cursor was before the press (`mouse`).
    fn reveal(&self, i: usize) -> Reveal {
        let at = self.reveal_at();
        if self.reader {
            Reveal::None
        } else if i == at.line {
            Reveal::Cursor(at.col)
        } else if self.group(i).is_some() && self.group(i) == self.group(at.line) {
            Reveal::Block
        } else {
            Reveal::None
        }
    }
}

impl ViewSource for Editor {
    fn line_count(&self) -> usize {
        self.buf.line_count()
    }
    fn line(&self, i: usize) -> &str {
        self.buf.line(i)
    }
    /// The line's display: inline elements from `parse_line`, the code
    /// background from `code_bg`, ordered numbers from `ordered_display`, and
    /// a code line's language from its block.
    fn display(&self, i: usize) -> LineDisplay {
        let line = self.buf.line(i);
        let kind = self.blocks.kind(i);
        let mut d = line_display(line, kind, &parse_line(line, kind), self.reveal(i));
        match kind {
            LineKind::FenceOpen { .. } | LineKind::FenceClose { .. } => {
                d.deco = code_bg(&self.blocks, i);
            }
            LineKind::Code { block } => {
                d.deco = code_bg(&self.blocks, i);
                let lang = &self.blocks.code_block(*block).lang;
                for s in &mut d.spans {
                    if let SpanBase::CodeBlock { lang: l } = &mut s.style.base {
                        l.clone_from(lang);
                    }
                }
            }
            _ => {}
        }
        if let Prefix::Number(n) = &mut d.prefix {
            *n = ordered_display(&self.blocks, &self.buf, i);
        }
        d
    }
    fn cursor(&self) -> Pos {
        self.cursor
    }
    fn status(&self) -> StatusModel {
        // The find bar (vim off) keeps its line while a message shows: the
        // message goes on the right, before the counter.
        let find = self.find_counter().filter(|_| self.asking.is_none());
        let left = if find.is_some() {
            let query = self.search.as_ref().map_or("", |s| s.query.as_str());
            let (pre, at) = self.preedit().unwrap_or_default();
            StatusLeft::Input {
                prefix: strings::FIND_PROMPT,
                text: format!("{query}{pre}"),
                cursor: query.len() + at,
            }
        } else if let Some((text, kind)) = &self.message {
            StatusLeft::Message {
                text: text.clone(),
                kind: *kind,
            }
        } else if let Some(p) = self.prompt() {
            StatusLeft::Prompt(p)
        } else {
            let typed = match (self.mode, &self.searching) {
                (Mode::Command, _) if self.linking() => Some((strings::LINK_PROMPT, &self.cmdline)),
                (Mode::Command, _) => Some((":", &self.cmdline)),
                (Mode::Search, Some(s)) => {
                    Some((if s.search.forward { "/" } else { "?" }, &s.search.query))
                }
                _ => None,
            };
            match typed {
                Some((prefix, text)) => {
                    // A composition shows after what is typed.
                    let (pre, at) = self.preedit().unwrap_or_default();
                    StatusLeft::Input {
                        prefix,
                        text: format!("{text}{pre}"),
                        cursor: text.len() + at,
                    }
                }
                // Reader view names itself in place of Normal (or Insert,
                // with vim off); a selection made in it keeps its name.
                None if self.reader && matches!(self.mode, Mode::Normal | Mode::Insert) => {
                    StatusLeft::Mode(strings::MODE_READER)
                }
                // Vim off: no mode to name.
                None if self.plain_editing() => StatusLeft::Mode(""),
                None => StatusLeft::Mode(self.mode.name()),
            }
        };
        // Reader view hides the cursor, so its position would mislead.
        let position = if self.reader {
            String::new()
        } else {
            status::position(&self.buf, self.cursor)
        };
        StatusModel {
            left,
            file: self.title.clone(),
            dirty: self.dirty,
            warnings: self.shown_warnings(),
            showcmd: find.unwrap_or_else(|| self.showcmd()),
            position,
        }
    }
    fn tokens(&self) -> &Tokens {
        &self.tokens
    }
    fn theme(&self) -> Theme {
        self.theme
    }
    fn reader(&self) -> bool {
        self.reader
    }
    fn help(&self) -> Option<usize> {
        self.help
    }
    fn picker(&self) -> Option<&TablePicker> {
        self.picker.as_ref()
    }
    fn selection(&self) -> Option<Selection> {
        self.shown_selection()
    }
    fn matches(&self, i: usize) -> &[Range<usize>] {
        self.search().map_or(&[], |s| s.matches_on(&self.buf, i))
    }
    /// While `:s///c` asks, the match it asks about (which can be one the
    /// smartcase highlights do not show), else the search's.
    fn current_match(&self) -> Option<(usize, Range<usize>)> {
        match &self.confirm {
            Some(session) => session.current(),
            None => self.search()?.current(),
        }
    }
    fn caret_shape(&self) -> CaretShape {
        match self.mode {
            _ if self.reader => CaretShape::Hidden,
            // The find bar's caret is in the status bar.
            _ if self.finding() => CaretShape::Hidden,
            // Vim off, read-only: nothing claims typing goes in.
            Mode::Insert if !self.cfg.vim && self.refuses_edits() => CaretShape::Block,
            Mode::Insert => CaretShape::Bar,
            Mode::Search | Mode::Command => CaretShape::Hidden,
            Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::Confirm | Mode::Prompt => {
                CaretShape::Block
            }
        }
    }
    /// Only Insert's composition shows in the document; the command line's
    /// and a search's show in the status bar (`status`).
    fn preedit(&self) -> Option<(&str, usize)> {
        let typing =
            !self.finding() && ((!self.cfg.vim && !self.linking()) || self.mode == Mode::Insert);
        Editor::preedit(self).filter(|_| typing)
    }
    fn line_shifts_since(&self, since: Option<ShiftMark>) -> Option<LineShifts> {
        Some(LineShifts {
            mark: self.buf.shift_mark(),
            shifts: since.and_then(|m| self.buf.shifts_since(m)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::SignalKind;
    use crate::markdown::{
        BlockDeco, Prefix, Reveal, SpanBase, line_display, parse_inline, parse_line,
    };
    use crate::strings;
    use crate::testkit::{FakeHost, build, key, req};

    /// The insert path on its own: vim off.
    fn ed(text: &str) -> Editor {
        build(
            text,
            Config {
                vim: false,
                ..Config::default()
            },
            false,
        )
    }

    fn at(e: &mut Editor, line: usize, col: usize) {
        e.cursor = Pos { line, col };
    }

    fn type_str(e: &mut Editor, s: &str) -> Effects {
        let mut fx = Effects::default();
        for c in s.chars() {
            let k = if c == ' ' { Key::Space } else { Key::Char(c) };
            let out = e.handle(key(k));
            fx.redraw = std::mem::take(&mut fx.redraw).merge(out.redraw);
            fx.dirty = fx.dirty.or(out.dirty);
        }
        fx
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("abc_def-editor-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `Editor::open` on `r`, with the state directory in `dir` rather
    /// than the one the environment names.
    fn open_in(r: OpenRequest, dir: &std::path::Path) -> Editor {
        let state = StateDir::at(dir.join("state"));
        Editor::open_with_state(
            r,
            Config::default(),
            Platform::Mac,
            Box::new(FakeHost::default()),
            Some(&state),
        )
    }

    #[test]
    fn typing_inserts_and_marks_dirty() {
        let mut e = ed("hello");
        assert!(!e.dirty());
        at(&mut e, 0, 5);
        let fx = type_str(&mut e, " you");
        assert_eq!(e.text(), "hello you");
        assert_eq!(e.cursor(), Pos { line: 0, col: 9 });
        assert!(e.dirty());
        assert_eq!(fx.dirty, Some(true));
        // Already dirty: the next edit does not report it again.
        assert_eq!(e.handle(key(Key::Char('!'))).dirty, None);
        assert!(e.status().dirty);
    }

    #[test]
    fn text_input_inserts_committed_text() {
        let mut e = ed("ab");
        at(&mut e, 0, 1);
        e.handle(Input::Text("日本".into()));
        assert_eq!(e.text(), "a日本b");
        assert_eq!(e.cursor(), Pos { line: 0, col: 7 });
    }

    #[test]
    fn shortcut_chars_do_not_insert() {
        // A chord the spec leaves free: not Primary+S (`:w`), nor the
        // link keys K and L, nor B, I, Z, F, W, Q, C, X or V.
        let mut e = ed("ab");
        let fx = e.handle(Input::Key(KeyEvent {
            key: Key::Char('j'),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
            time_ns: 0,
        }));
        assert_eq!(e.text(), "ab");
        assert_eq!(fx, Effects::default());
        e.handle(Input::Key(KeyEvent {
            key: Key::Char('s'),
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            time_ns: 0,
        }));
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn backspace_at_col0_joins() {
        let mut e = ed("one\ntwo\nthree");
        at(&mut e, 1, 0);
        let fx = e.handle(key(Key::Backspace));
        assert_eq!(e.text(), "onetwo\nthree");
        assert_eq!(e.cursor(), Pos { line: 0, col: 3 });
        assert_eq!(fx.dirty, Some(true));
        assert!(fx.scroll_to_cursor);
        // At the very start there is nothing to join.
        at(&mut e, 0, 0);
        assert_eq!(e.handle(key(Key::Backspace)).dirty, None);
        assert_eq!(e.text(), "onetwo\nthree");
    }

    #[test]
    fn delete_at_line_end_joins_the_next_line() {
        let mut e = ed("ab\ncd");
        at(&mut e, 0, 2);
        e.handle(key(Key::Delete));
        assert_eq!(e.text(), "abcd");
        assert_eq!(e.cursor(), Pos { line: 0, col: 2 });
        e.handle(key(Key::Delete));
        assert_eq!(e.text(), "abd");
    }

    #[test]
    fn enter_splits_a_plain_line_and_continues_a_list() {
        let mut e = ed("item one");
        at(&mut e, 0, 4);
        e.handle(key(Key::Enter));
        assert_eq!(e.text(), "item\n one");
        assert_eq!(e.cursor(), Pos { line: 1, col: 0 });
        // On a list item the new line takes the item's prefix.
        let mut e = ed("- item one");
        at(&mut e, 0, 6);
        e.handle(key(Key::Enter));
        assert_eq!(e.text(), "- item\n-  one");
        assert_eq!(e.cursor(), Pos { line: 1, col: 2 });
    }

    #[test]
    fn edits_and_motions_never_split_graphemes() {
        // A decomposed accent, then a ZWJ family emoji, then x.
        let s = "e\u{301}👩\u{200d}👩\u{200d}👧x";
        let mut e = ed(s);
        let mut cols = vec![0];
        for _ in 0..4 {
            e.handle(key(Key::Right));
            cols.push(e.cursor().col);
        }
        assert_eq!(cols, [0, 3, 21, 22, 22]);
        for _ in 0..3 {
            e.handle(key(Key::Left));
        }
        assert_eq!(e.cursor().col, 0);

        // Backspace from just after the emoji takes the whole family, then
        // the accent with its base.
        at(&mut e, 0, 21);
        e.handle(key(Key::Backspace));
        assert_eq!(e.text(), "e\u{301}x");
        assert_eq!(e.cursor().col, 3);
        e.handle(key(Key::Backspace));
        assert_eq!(e.text(), "x");
        assert_eq!(e.cursor().col, 0);

        // Delete takes a whole cluster too.
        let mut e = ed(s);
        at(&mut e, 0, 3);
        e.handle(key(Key::Delete));
        assert_eq!(e.text(), "e\u{301}x");
        // A cursor left mid-cluster snaps back before inserting.
        let mut e = ed(s);
        at(&mut e, 0, 7);
        e.handle(key(Key::Char('y')));
        assert_eq!(e.text(), "e\u{301}y👩\u{200d}👩\u{200d}👧x");
    }

    #[test]
    fn up_and_down_keep_the_grapheme_column() {
        let mut e = ed("e\u{301}bcd\nxy\n👩\u{200d}👩\u{200d}👧abc");
        at(&mut e, 0, 5); // after e-acute and b, c: grapheme column 3
        e.handle(key(Key::Down));
        assert_eq!(e.cursor(), Pos { line: 1, col: 2 });
        e.handle(key(Key::Down));
        assert_eq!(e.cursor(), Pos { line: 2, col: 20 });
        e.handle(key(Key::Up));
        e.handle(key(Key::Up));
        assert_eq!(e.cursor(), Pos { line: 0, col: 5 });
        e.handle(key(Key::Up));
        assert_eq!(e.cursor(), Pos { line: 0, col: 5 });
    }

    #[test]
    fn left_and_right_wrap_between_lines() {
        let mut e = ed("ab\ncd");
        at(&mut e, 1, 0);
        e.handle(key(Key::Left));
        assert_eq!(e.cursor(), Pos { line: 0, col: 2 });
        e.handle(key(Key::Right));
        assert_eq!(e.cursor(), Pos { line: 1, col: 0 });
        e.handle(key(Key::End));
        assert_eq!(e.cursor(), Pos { line: 1, col: 2 });
        e.handle(key(Key::Home));
        assert_eq!(e.cursor(), Pos { line: 1, col: 0 });
    }

    #[test]
    fn effects_redraw_only_edited_line() {
        let mut e = ed("first\n\nsecond line\n\nthird");
        at(&mut e, 2, 6);
        let fx = e.handle(key(Key::Char('x')));
        assert_eq!(fx.redraw, Redraw::Lines(2..3));
        assert!(fx.scroll_to_cursor);
        let fx = e.handle(key(Key::Backspace));
        assert_eq!(fx.redraw, Redraw::Lines(2..3));
        let fx = e.handle(key(Key::Right));
        assert_eq!(fx.redraw, Redraw::Lines(2..3));
    }

    #[test]
    fn leaving_a_code_block_redraws_the_whole_block() {
        let mut e = ed("```\na\nb\n```\nafter");
        at(&mut e, 3, 3);
        let fx = e.handle(key(Key::Down));
        assert_eq!(e.cursor(), Pos { line: 4, col: 3 });
        assert_eq!(fx.redraw, Redraw::Lines(0..5));
    }

    #[test]
    fn typing_a_fence_redraws_the_lines_it_turns_to_code() {
        let mut e = ed("``\na\nb");
        at(&mut e, 0, 2);
        let fx = e.handle(key(Key::Char('`')));
        assert_eq!(fx.redraw, Redraw::Lines(0..3));
    }

    #[test]
    fn bench_mode_starts_at_first_bold_paragraph() {
        let text = "# **Heading** bold\n\n- **item**\n\nplain\nsome **bold** text";
        let e = build(text, Config::default(), true);
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), Pos { line: 5, col: 7 });
        assert_eq!(e.status().left, StatusLeft::Mode("INSERT"));
    }

    #[test]
    fn bench_mode_without_bold_starts_at_the_document_start() {
        let e = build("plain\ntext", Config::default(), true);
        assert_eq!(e.mode(), Mode::Insert);
        assert_eq!(e.cursor(), Pos::default());
    }

    #[test]
    fn missing_file_opens_empty_with_name() {
        let dir = scratch("missing");
        let path = dir.join("nothing-here.md");
        let e = open_in(req(path.to_str().unwrap()), &dir);
        assert_eq!(e.title(), "nothing-here.md");
        assert_eq!(e.text(), "");
        assert_eq!(e.line_count(), 1);
        assert_eq!(e.message(), None);
        assert!(!e.dirty());
        assert_eq!(e.status().file, "nothing-here.md");
    }

    #[test]
    fn existing_file_opens_with_its_text() {
        let dir = scratch("existing");
        let path = dir.join("doc.md");
        std::fs::write(&path, "# Title\n\nbody\n").unwrap();
        let mut r = req(path.to_str().unwrap());
        r.display_name = Some("Shown Name".into());
        let e = open_in(r, &dir);
        assert_eq!(e.text(), "# Title\n\nbody\n");
        assert_eq!(e.line_count(), 3);
        assert_eq!(e.title(), "Shown Name");
        assert_eq!(e.status().file, "Shown Name");
    }

    #[test]
    fn unreadable_file_opens_empty_with_a_message() {
        // A directory exists but cannot be read as text.
        let dir = scratch("unreadable");
        let e = open_in(req(dir.to_str().unwrap()), &dir);
        assert_eq!(e.text(), "");
        assert_eq!(e.message(), Some(strings::READ_FAILED));
    }

    #[test]
    fn config_defaults_to_vim() {
        assert!(Config::default().vim);
    }

    #[test]
    fn status_shows_mode_file_and_grapheme_position() {
        // Vim off: no mode to name (`nonvim`).
        let mut e = ed("e\u{301}x");
        at(&mut e, 0, 3);
        let s = e.status();
        assert_eq!(s.left, StatusLeft::Mode(""));
        assert_eq!(s.file, "notes.md");
        assert!(!s.dirty);
        assert_eq!(s.position, "1:2");
    }

    #[test]
    fn reader_mode_ignores_edits_and_reveals_nothing() {
        let mut e = ed("**b**");
        e.reader = true;
        let fx = e.handle(key(Key::Char('x')));
        assert_eq!(e.text(), "**b**");
        assert_eq!(fx.dirty, None);
        assert_eq!(e.reveal(0), Reveal::None);
    }

    #[test]
    fn vim_off_read_only_and_reader_take_no_edits_nor_claim_insert() {
        for which in ["read-only", "reader"] {
            let (mut e, host) = crate::testkit::plain_with_host("- [ ] ab");
            host.set_clipboard("XY");
            match which {
                "read-only" => e.read_only_chosen = true,
                _ => e.reader = true,
            }
            crate::testkit::keys(
                &mut e,
                "<End>x<CR><BS><Del><Tab><S-Tab><C-Space><D-b><D-i><D-v><D-x><D-z>",
            );
            e.handle(Input::Text("typed".into()));
            assert_eq!(e.text(), "- [ ] ab", "{which}");
            assert!(!e.dirty(), "{which}");
            assert_ne!(e.caret_shape(), CaretShape::Bar, "{which}");
            assert_ne!(e.status().left, StatusLeft::Mode("INSERT"), "{which}");
        }
    }

    #[test]
    fn close_requested_closes_with_code_zero() {
        let mut e = ed("");
        assert_eq!(e.handle(Input::CloseRequested).close, Some(0));
    }

    #[test]
    fn terminate_closes_with_143() {
        let mut e = ed("");
        assert_eq!(
            e.handle(Input::Signal(SignalKind::Terminate)).close,
            Some(143)
        );
    }

    #[test]
    fn hangup_closes_with_129() {
        let mut e = ed("");
        assert_eq!(e.handle(Input::Signal(SignalKind::Hangup)).close, Some(129));
    }

    #[test]
    fn interrupt_closes_with_130() {
        let mut e = ed("");
        assert_eq!(
            e.handle(Input::Signal(SignalKind::Interrupt)).close,
            Some(130)
        );
    }

    // ViewSource::display: how each line's display is assembled.

    #[test]
    fn display_parses_inline_after_the_prefix() {
        let text = "* a *b*";
        let mut e = ed(&format!("x\n{text}"));
        at(&mut e, 0, 0);
        let kind = e.blocks.kind(1).clone();
        let d = e.display(1);
        assert_eq!(
            d,
            line_display(text, &kind, &parse_line(text, &kind), Reveal::None)
        );
        assert_ne!(
            d,
            line_display(text, &kind, &parse_inline(text), Reveal::None)
        );
        assert!(d.spans.iter().any(|s| s.src == (5..6) && s.style.em));
    }

    #[test]
    fn display_takes_code_bg_from_the_blocks() {
        // Unclosed: line_display alone would say `unclosed: false`.
        let mut e = ed("x\n```rust\nfn f()");
        at(&mut e, 0, 0);
        assert_eq!(
            e.display(2).deco,
            BlockDeco::CodeBg {
                first: false,
                last: true,
                unclosed: true
            }
        );
        assert_eq!(
            e.display(1).deco,
            BlockDeco::CodeBg {
                first: true,
                last: false,
                unclosed: true
            }
        );
    }

    #[test]
    fn display_names_the_code_block_language() {
        let mut e = ed("x\n```rust\nfn f()\n```");
        at(&mut e, 0, 0);
        let d = e.display(2);
        assert!(!d.spans.is_empty());
        for s in &d.spans {
            assert_eq!(
                s.style.base,
                SpanBase::CodeBlock {
                    lang: Some("rust".into())
                }
            );
        }
    }

    #[test]
    fn display_numbers_ordered_items_from_the_blocks() {
        let mut e = ed("x\n1. a\n1. b\n  1. c");
        at(&mut e, 0, 0);
        assert_eq!(e.display(2).prefix, Prefix::Number("2.".into()));
        assert_eq!(e.display(3).prefix, Prefix::Number("a.".into()));
    }

    #[test]
    fn reveal_follows_the_cursor() {
        let mut e = ed("para **b**\n```\ncode\n```\n| a |\n|---|\n| 1 |\nend");
        at(&mut e, 0, 6);
        assert_eq!(e.reveal(0), Reveal::Cursor(6));
        assert_eq!(e.reveal(1), Reveal::None);
        assert_eq!(e.reveal(4), Reveal::None);
        // The cursor line shows its element raw.
        assert!(
            e.display(0)
                .spans
                .iter()
                .any(|s| s.style.base == SpanBase::Syntax)
        );

        at(&mut e, 2, 1);
        assert_eq!(e.reveal(2), Reveal::Cursor(1));
        assert_eq!(e.reveal(1), Reveal::Block);
        assert_eq!(e.reveal(3), Reveal::Block);
        assert_eq!(e.reveal(0), Reveal::None);
        assert_eq!(e.reveal(4), Reveal::None);

        at(&mut e, 6, 0);
        assert_eq!(e.reveal(4), Reveal::Block);
        assert_eq!(e.reveal(5), Reveal::Block);
        assert_eq!(e.reveal(3), Reveal::None);
        assert_eq!(e.reveal(7), Reveal::None);
    }

    /// Asserts the reveal extent of each listed line.
    fn extents(text: &str, want: &[(usize, Range<usize>)]) {
        let e = ed(text);
        for (line, r) in want {
            assert_eq!(e.reveal_extent(*line), *r, "line {line} of {text:?}");
        }
    }

    #[test]
    fn reveal_extent_of_an_unclosed_block_runs_to_the_end() {
        extents(
            "a\n```\nx\ny",
            &[(0, 0..1), (1, 1..4), (2, 1..4), (3, 1..4)],
        );
    }

    #[test]
    fn reveal_extent_of_an_unclosed_block_stops_at_the_next_opener() {
        let text = "```\na\n```rust\nb\n```\nafter";
        extents(
            text,
            &[
                (0, 0..2),
                (1, 0..2),
                (2, 2..5),
                (3, 2..5),
                (4, 2..5),
                (5, 5..6),
            ],
        );
    }

    #[test]
    fn reveal_extent_reads_the_infos() {
        extents(
            "a\n```\nx\ny\n```\nb",
            &[
                (0, 0..1),
                (1, 1..5),
                (2, 1..5),
                (3, 1..5),
                (4, 1..5),
                (5, 5..6),
            ],
        );
        extents(
            "p\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\nq",
            &[
                (0, 0..1),
                (1, 1..5),
                (2, 1..5),
                (3, 1..5),
                (4, 1..5),
                (5, 5..6),
            ],
        );
    }
}
