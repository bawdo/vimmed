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
        let mut parts = name.split('.');
        let head = parts.next().unwrap_or(name);
        Some(match head {
            // `conditional`, `storageclass` and the CSS at-rule names are
            // keyword-like captures used by the SQL and CSS grammars.
            "keyword" | "conditional" | "storageclass" | "charset" | "import" | "keyframes"
            | "media" | "namespace" | "supports" => SyntaxClass::Keyword,
            "string" | "escape" => SyntaxClass::String,
            "comment" => SyntaxClass::Comment,
            "function" => SyntaxClass::Function,
            "type" | "constructor" => SyntaxClass::Type,
            "number" | "float" => SyntaxClass::Number,
            "constant" | "boolean" => SyntaxClass::Constant,
            "operator" => SyntaxClass::Operator,
            "punctuation" => SyntaxClass::Punctuation,
            "variable" | "parameter" => SyntaxClass::Variable,
            "property" | "field" => SyntaxClass::Property,
            "attribute" => SyntaxClass::Attribute,
            "tag" => SyntaxClass::Tag,
            // The markdown grammars' prose captures.
            "text" => match parts.next() {
                Some("title") => SyntaxClass::Keyword,
                Some("literal") => SyntaxClass::String,
                Some("uri") => SyntaxClass::Constant,
                Some("reference") => SyntaxClass::Function,
                Some("emphasis") | Some("strong") => SyntaxClass::Variable,
                _ => return None,
            },
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

/// A language a code block can be highlighted as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Go,
    JavaScript,
    TypeScript,
    Tsx,
    Ruby,
    Sql,
    Html,
    Css,
    Bash,
    Hcl,
    /// Block grammar plus the inline grammar injected over its `inline` nodes.
    /// Used only to colour code blocks tagged `md`/`markdown`, never to render
    /// the document itself.
    Markdown,
}

/// Resolve a fenced-code info tag (case-insensitive) to a language.
pub fn language_for(tag: &str) -> Option<Lang> {
    Some(match tag.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" => Lang::Rust,
        "go" | "golang" => Lang::Go,
        "javascript" | "js" | "jsx" => Lang::JavaScript,
        "typescript" | "ts" => Lang::TypeScript,
        "tsx" => Lang::Tsx,
        "ruby" | "rb" => Lang::Ruby,
        "sql" => Lang::Sql,
        "html" => Lang::Html,
        "css" => Lang::Css,
        "bash" | "sh" | "shell" | "zsh" => Lang::Bash,
        "hcl" | "tf" | "terraform" => Lang::Hcl,
        "md" | "markdown" => Lang::Markdown,
        _ => return None,
    })
}

/// A lazily compiled, process-lifetime configuration. Injections and locals
/// are empty: only markdown needs injection, and it is handled in
/// `inline_caps`.
macro_rules! lazy_config {
    ($name:literal, $language:expr, $highlights:expr) => {{
        static CONFIG: OnceLock<HighlightConfiguration> = OnceLock::new();
        CONFIG.get_or_init(|| {
            HighlightConfiguration::new($language.into(), $name, &$highlights, "", "")
                .expect(concat!("the bundled ", $name, " highlights query compiles"))
        })
    }};
}

/// The highlights query for HCL. The grammar crate ships none, so this is a
/// small one covering the constructs a code block shows.
const HCL_HIGHLIGHTS: &str = r#"
(comment) @comment
(block . (identifier) @keyword)
(attribute (identifier) @property)
(string_lit) @string
(quoted_template) @string
(heredoc_template) @string
(numeric_lit) @number
(bool_lit) @constant
(null_lit) @constant
(function_call (identifier) @function)
["for" "in" "if" "else" "endfor" "endif"] @keyword
["=" "==" "!=" "<" "<=" ">" ">=" "+" "-" "*" "/" "%" "!" "&&" "||" "?" "=>"] @operator
["{" "}" "[" "]" "(" ")" "," "."] @punctuation
"#;

impl Lang {
    /// The compiled configuration, built on first use (so on the worker) and
    /// kept for the life of the process.
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
            Lang::Go => lazy_config!(
                "go",
                tree_sitter_go::LANGUAGE,
                tree_sitter_go::HIGHLIGHTS_QUERY
            ),
            Lang::JavaScript => lazy_config!(
                "javascript",
                tree_sitter_javascript::LANGUAGE,
                format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
                )
            ),
            // The TypeScript query only adds to the JavaScript one.
            Lang::TypeScript => lazy_config!(
                "typescript",
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
                format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                )
            ),
            Lang::Tsx => lazy_config!(
                "tsx",
                tree_sitter_typescript::LANGUAGE_TSX,
                format!(
                    "{}\n{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                )
            ),
            Lang::Ruby => lazy_config!(
                "ruby",
                tree_sitter_ruby::LANGUAGE,
                tree_sitter_ruby::HIGHLIGHTS_QUERY
            ),
            Lang::Sql => lazy_config!(
                "sql",
                tree_sitter_sequel::LANGUAGE,
                tree_sitter_sequel::HIGHLIGHTS_QUERY
            ),
            Lang::Html => lazy_config!(
                "html",
                tree_sitter_html::LANGUAGE,
                tree_sitter_html::HIGHLIGHTS_QUERY
            ),
            Lang::Css => lazy_config!(
                "css",
                tree_sitter_css::LANGUAGE,
                tree_sitter_css::HIGHLIGHTS_QUERY
            ),
            Lang::Bash => lazy_config!(
                "bash",
                tree_sitter_bash::LANGUAGE,
                tree_sitter_bash::HIGHLIGHT_QUERY
            ),
            Lang::Hcl => lazy_config!("hcl", tree_sitter_hcl::LANGUAGE, HCL_HIGHLIGHTS),
            Lang::Markdown => lazy_config!(
                "markdown",
                tree_sitter_md::LANGUAGE,
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK
            ),
        }
    }

    /// The injected inline grammar's configuration (markdown only).
    fn inline_config(self) -> Option<&'static HighlightConfiguration> {
        match self {
            Lang::Markdown => Some(lazy_config!(
                "markdown_inline",
                tree_sitter_md::INLINE_LANGUAGE,
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE
            )),
            _ => None,
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
        let Some(lang) = language_for(lang) else {
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
    let lines = spans_for(lang, config, &tree, text);
    trees.insert(block, Parsed { lang, tree });
    Some((lines, info))
}

/// (start byte, end byte, pattern index, class)
type Cap = (usize, usize, usize, SyntaxClass);

/// Run `config`'s highlights query over `tree` and push the mapped captures.
/// `pattern_base` offsets pattern indices so a second grammar's patterns
/// rank after the first's.
fn collect_caps(
    config: &HighlightConfiguration,
    tree: &Tree,
    text: &str,
    pattern_base: usize,
    caps: &mut Vec<Cap>,
) {
    let names = config.query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&config.query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for c in m.captures() {
            if let Some(class) = SyntaxClass::from_capture(names[c.index as usize]) {
                let n = c.node;
                caps.push((
                    n.start_byte(),
                    n.end_byte(),
                    pattern_base + m.pattern_index,
                    class,
                ));
            }
        }
    }
}

/// Markdown injection: the block parse reports each `inline` node's range;
/// parse exactly those ranges with the inline grammar and run its query.
/// (Task 14's loop does not run injection queries, so this is the one
/// injection we do by hand.) A failed inline parse leaves the block colours.
fn inline_caps(
    block: &HighlightConfiguration,
    inline: &HighlightConfiguration,
    tree: &Tree,
    text: &str,
    caps: &mut Vec<Cap>,
) {
    let mut ranges = Vec::new();
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        if node.kind() == "inline" {
            ranges.push(node.range());
        } else if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    if ranges.is_empty() {
        return;
    }
    let mut parser = Parser::new();
    if parser.set_language(&inline.language).is_err()
        || parser.set_included_ranges(&ranges).is_err()
    {
        return;
    }
    let Some(inline_tree) = parser.parse(text, None) else {
        return;
    };
    collect_caps(
        inline,
        &inline_tree,
        text,
        block.query.pattern_count(),
        caps,
    );
}

/// Run the highlights query over `tree` and cut the result into per-line spans.
fn spans_for(
    lang: Lang,
    config: &HighlightConfiguration,
    tree: &Tree,
    text: &str,
) -> Vec<LineSpans> {
    // Paint widest captures first so the narrowest wins; for equal ranges the
    // earliest pattern wins (it is painted last).
    let mut caps: Vec<Cap> = Vec::new();
    collect_caps(config, tree, text, 0, &mut caps);
    if let Some(inline) = lang.inline_config() {
        inline_caps(config, inline, tree, text, &mut caps);
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
