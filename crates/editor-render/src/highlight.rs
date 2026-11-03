//! Lazy, incremental code-block highlighting.
//!
//! A single worker thread (started on the first request for a known language)
//! owns every block's parse `Tree`. Compiling a language's
//! `HighlightConfiguration` and highlighting both happen there, never on the
//! caller's thread, and `poll` never blocks. First paint is plain; colour
//! arrives on a later frame as spans, which never change a line's metrics.
//!
//! We own the `Parser`/`Tree` loop (rather than using the one-shot
//! `tree_sitter_highlight::Highlighter`) so an edit re-parses against the old
//! tree. The configuration's compiled `Query` is then run over our tree.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};

use editor_core::host::Waker;
use editor_core::markdown::BlockId;
use editor_core::tokens::TokenId;
use tree_sitter::{InputEdit, Parser, QueryCursor, StreamingIterator, Tree};
use tree_sitter_highlight::HighlightConfiguration;

/// A highlight class. One-to-one with the `color.syntax.*` code tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxClass {
    Keyword,
    String,
    Comment,
    Function,
    Type,
    Number,
    Constant,
    Operator,
    Punctuation,
    Variable,
    Property,
    Attribute,
    Tag,
}

impl SyntaxClass {
    pub const ALL: &'static [SyntaxClass] = &[
        SyntaxClass::Keyword,
        SyntaxClass::String,
        SyntaxClass::Comment,
        SyntaxClass::Function,
        SyntaxClass::Type,
        SyntaxClass::Number,
        SyntaxClass::Constant,
        SyntaxClass::Operator,
        SyntaxClass::Punctuation,
        SyntaxClass::Variable,
        SyntaxClass::Property,
        SyntaxClass::Attribute,
        SyntaxClass::Tag,
    ];

    /// The colour token for this class (looked up by the painter).
    pub fn token(self) -> TokenId {
        match self {
            SyntaxClass::Keyword => TokenId::ColorSyntaxKeyword,
            SyntaxClass::String => TokenId::ColorSyntaxString,
            SyntaxClass::Comment => TokenId::ColorSyntaxComment,
            SyntaxClass::Function => TokenId::ColorSyntaxFunction,
            SyntaxClass::Type => TokenId::ColorSyntaxType,
            SyntaxClass::Number => TokenId::ColorSyntaxNumber,
            SyntaxClass::Constant => TokenId::ColorSyntaxConstant,
            SyntaxClass::Operator => TokenId::ColorSyntaxOperator,
            SyntaxClass::Punctuation => TokenId::ColorSyntaxPunctuation,
            SyntaxClass::Variable => TokenId::ColorSyntaxVariable,
            SyntaxClass::Property => TokenId::ColorSyntaxProperty,
            SyntaxClass::Attribute => TokenId::ColorSyntaxAttribute,
            SyntaxClass::Tag => TokenId::ColorSyntaxTag,
        }
    }

    /// Map a tree-sitter capture name (`function.method`, `punctuation.bracket`)
    /// by its leading segment. Unmapped captures stay plain.
    pub fn from_capture(name: &str) -> Option<SyntaxClass> {
        let head = name.split('.').next().unwrap_or(name);
        Some(match head {
            "keyword" => SyntaxClass::Keyword,
            "string" | "escape" => SyntaxClass::String,
            "comment" => SyntaxClass::Comment,
            "function" => SyntaxClass::Function,
            "type" | "constructor" => SyntaxClass::Type,
            "number" => SyntaxClass::Number,
            "constant" => SyntaxClass::Constant,
            "operator" => SyntaxClass::Operator,
            "punctuation" => SyntaxClass::Punctuation,
            "variable" => SyntaxClass::Variable,
            "property" => SyntaxClass::Property,
            "attribute" => SyntaxClass::Attribute,
            "tag" => SyntaxClass::Tag,
            _ => return None,
        })
    }
}

/// Spans on one line: byte ranges relative to the line start.
pub type LineSpans = Vec<(Range<usize>, SyntaxClass)>;

/// What the last parse of a block did, for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReparseInfo {
    /// True when the previous tree was handed to the parser.
    pub incremental: bool,
    /// Total bytes the parser reported as changed against the old tree
    /// (zero for a first parse).
    pub changed_bytes: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
}

impl Lang {
    fn from_name(name: &str) -> Option<Lang> {
        match name.trim().to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(Lang::Rust),
            _ => None,
        }
    }

    /// The compiled configuration, built on first use (so on the worker) and
    /// kept for the life of the process. Locals are empty.
    fn config(self) -> &'static HighlightConfiguration {
        match self {
            Lang::Rust => {
                static CONFIG: OnceLock<HighlightConfiguration> = OnceLock::new();
                CONFIG.get_or_init(|| {
                    HighlightConfiguration::new(
                        tree_sitter_rust::LANGUAGE.into(),
                        "rust",
                        tree_sitter_rust::HIGHLIGHTS_QUERY,
                        tree_sitter_rust::INJECTIONS_QUERY,
                        "",
                    )
                    .expect("the bundled Rust highlights query compiles")
                })
            }
        }
    }
}

enum Job {
    Request {
        block: BlockId,
        gen_: u64,
        lang: Lang,
        text: String,
    },
    Edit {
        block: BlockId,
        gen_: u64,
        edit: InputEdit,
        text: String,
    },
}

struct Done {
    block: BlockId,
    gen_: u64,
    lines: Vec<LineSpans>,
    info: ReparseInfo,
}

struct BlockState {
    /// Latest generation sent to the worker; older results are dropped.
    gen_: u64,
    /// The language and text last sent to the worker (by `request` or `edit`),
    /// so an unchanged `request` can be skipped.
    lang: Lang,
    text: String,
    lines: Vec<LineSpans>,
    info: Option<ReparseInfo>,
}

pub struct Highlighter {
    waker: Waker,
    worker: Option<Sender<Job>>,
    results: Option<Receiver<Done>>,
    blocks: HashMap<BlockId, BlockState>,
    next_gen: u64,
    marked: bool,
}

impl Highlighter {
    pub fn new(waker: Waker) -> Highlighter {
        Highlighter {
            waker,
            worker: None,
            results: None,
            blocks: HashMap::new(),
            next_gen: 0,
            marked: false,
        }
    }

    /// Ask for `text` to be highlighted as `lang`. Returns at once; the block
    /// is plain until a later `poll` reports it.
    ///
    /// The caller must request only blocks that are on screen. The highlighter
    /// does no work for blocks nobody requested, and none for an unknown
    /// language (which stays plain).
    ///
    /// Idempotent: a request whose language and text match the last request
    /// or `edit` for this block does nothing (no job, no new generation, no
    /// wake, tree untouched), so it is safe to call for every on-screen block
    /// on every paint. Changed text or language re-highlights from scratch.
    pub fn request(&mut self, block: BlockId, lang: &str, text: &str) {
        let Some(lang) = Lang::from_name(lang) else {
            return;
        };
        if let Some(s) = self.blocks.get(&block)
            && s.lang == lang
            && s.text == text
        {
            return;
        }
        let gen_ = self.bump();
        let state = self.blocks.entry(block).or_insert(BlockState {
            gen_,
            lang,
            text: String::new(),
            lines: Vec::new(),
            info: None,
        });
        state.gen_ = gen_;
        state.lang = lang;
        state.text = text.to_owned();
        self.send(Job::Request {
            block,
            gen_,
            lang,
            text: text.to_owned(),
        });
    }

    /// Blocks whose colour changed since the last poll. Never blocks.
    pub fn poll(&mut self) -> Vec<BlockId> {
        let mut changed = Vec::new();
        let Some(rx) = &self.results else {
            return changed;
        };
        while let Ok(done) = rx.try_recv() {
            let Some(state) = self.blocks.get_mut(&done.block) else {
                continue;
            };
            if done.gen_ != state.gen_ {
                continue; // superseded by a later edit
            }
            state.lines = done.lines;
            state.info = Some(done.info);
            if !changed.contains(&done.block) {
                changed.push(done.block);
            }
        }
        if !changed.is_empty() && !self.marked {
            self.marked = true;
            marks::mark(marks::names::HIGHLIGHT_READY);
        }
        changed
    }

    /// Spans for one line of a block, byte ranges relative to the line start.
    /// Empty (plain) until the block has been highlighted.
    pub fn spans(&self, block: BlockId, line_in_block: usize) -> &[(Range<usize>, SyntaxClass)] {
        self.blocks
            .get(&block)
            .and_then(|s| s.lines.get(line_in_block))
            .map_or(&[], |l| l.as_slice())
    }

    /// Apply `edit` to the block's tree and re-parse `new_text` against it.
    /// Ignored for blocks that were never successfully requested. Until the
    /// result lands, the previous spans stay in place.
    pub fn edit(&mut self, block: BlockId, edit: InputEdit, new_text: &str) {
        if !self.blocks.contains_key(&block) {
            return;
        }
        let gen_ = self.bump();
        let state = self.blocks.get_mut(&block).unwrap();
        state.gen_ = gen_;
        state.text = new_text.to_owned();
        self.send(Job::Edit {
            block,
            gen_,
            edit,
            text: new_text.to_owned(),
        });
    }

    /// What the most recently applied parse of `block` did.
    pub fn last_reparse(&self, block: BlockId) -> Option<ReparseInfo> {
        self.blocks.get(&block).and_then(|s| s.info)
    }

    fn bump(&mut self) -> u64 {
        self.next_gen += 1;
        self.next_gen
    }

    fn send(&mut self, job: Job) {
        if self.worker.is_none() {
            let (jobs_tx, jobs_rx) = channel();
            let (done_tx, done_rx) = channel();
            let waker = self.waker.clone();
            std::thread::Builder::new()
                .name("highlight".into())
                .spawn(move || run_worker(jobs_rx, done_tx, waker))
                .expect("spawn highlight worker");
            self.worker = Some(jobs_tx);
            self.results = Some(done_rx);
        }
        // The worker only exits when this sender drops, so a send cannot fail.
        let _ = self.worker.as_ref().unwrap().send(job);
    }
}

struct Parsed {
    lang: Lang,
    tree: Tree,
}

fn run_worker(jobs: Receiver<Job>, done: Sender<Done>, waker: Waker) {
    let mut parser = Parser::new();
    let mut trees: HashMap<BlockId, Parsed> = HashMap::new();
    while let Ok(job) = jobs.recv() {
        let result = match job {
            Job::Request {
                block,
                gen_,
                lang,
                text,
            } => {
                trees.remove(&block);
                reparse(&mut parser, &mut trees, block, lang, false, &text).map(|(lines, info)| {
                    Done {
                        block,
                        gen_,
                        lines,
                        info,
                    }
                })
            }
            Job::Edit {
                block,
                gen_,
                edit,
                text,
            } => match trees.get_mut(&block) {
                Some(p) => {
                    p.tree.edit(&edit);
                    let lang = p.lang;
                    reparse(&mut parser, &mut trees, block, lang, true, &text).map(
                        |(lines, info)| Done {
                            block,
                            gen_,
                            lines,
                            info,
                        },
                    )
                }
                None => None,
            },
        };
        if let Some(d) = result
            && done.send(d).is_err()
        {
            return;
        }
        waker();
    }
}

/// Parse `text`, reusing the block's existing (already edited) tree when
/// `incremental` is set, and store the new tree.
fn reparse(
    parser: &mut Parser,
    trees: &mut HashMap<BlockId, Parsed>,
    block: BlockId,
    lang: Lang,
    incremental: bool,
    text: &str,
) -> Option<(Vec<LineSpans>, ReparseInfo)> {
    let config = lang.config();
    parser.set_language(&config.language).ok()?;
    let old = if incremental {
        trees.remove(&block)
    } else {
        None
    };
    let tree = parser.parse(text, old.as_ref().map(|p| &p.tree))?;
    let changed_bytes = old.as_ref().map_or(0, |p| {
        p.tree
            .changed_ranges(&tree)
            .map(|r| r.end_byte - r.start_byte)
            .sum()
    });
    let info = ReparseInfo {
        incremental: old.is_some(),
        changed_bytes,
    };
    let lines = spans_for(config, &tree, text);
    trees.insert(block, Parsed { lang, tree });
    Some((lines, info))
}

/// Run the highlights query over `tree` and cut the result into per-line spans.
fn spans_for(config: &HighlightConfiguration, tree: &Tree, text: &str) -> Vec<LineSpans> {
    let names = config.query.capture_names();
    // Paint widest captures first so the narrowest wins; for equal ranges the
    // earliest pattern wins (it is painted last).
    let mut caps: Vec<(usize, usize, usize, SyntaxClass)> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&config.query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for c in m.captures() {
            if let Some(class) = SyntaxClass::from_capture(names[c.index as usize]) {
                let n = c.node;
                caps.push((n.start_byte(), n.end_byte(), m.pattern_index, class));
            }
        }
    }
    caps.sort_by(|a, b| {
        (b.1 - b.0)
            .cmp(&(a.1 - a.0))
            .then(b.2.cmp(&a.2))
            .then(a.0.cmp(&b.0))
    });
    let mut paint: Vec<Option<SyntaxClass>> = vec![None; text.len()];
    for (s, e, _, class) in caps {
        for slot in &mut paint[s..e.min(text.len())] {
            *slot = Some(class);
        }
    }

    let mut lines = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        let mut spans: LineSpans = Vec::new();
        let mut i = 0;
        while i < line.len() {
            let Some(class) = paint[start + i] else {
                i += 1;
                continue;
            };
            let from = i;
            while i < line.len() && paint[start + i] == Some(class) {
                i += 1;
            }
            spans.push((from..i, class));
        }
        lines.push(spans);
        start += line.len() + 1;
    }
    lines
}
