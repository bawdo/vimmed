# Hanging block prefixes (November 2026)

The stack design left one option for the builder to evaluate: hang block prefixes (`#`, `>`, list
markers) in the left margin, as iA Writer does, so that revealing a prefix never moves the text.
Task 27 built it behind the token `size.hanging_prefix` (the width of the margin gutter the
prefixes hang in; 0 turns it off) and compared the two.

## Verdict

**Keep it off by default (`size.hanging_prefix = 0`). It works, and it is a one-line config change
for anyone who wants it; the owner may want to look at the pairs below and flip the default.**

Reasoning:

- **It does what it promises.** With it on, moving the cursor onto a heading, list item or quote
  leaves every glyph of the text where it was: the revealed `##`, `-` or `>` appears in the margin
  instead. With it off, revealing shifts the line: a heading by the width of `## ` (about 29 pt at
  `font.size.h2`), a list item left by about 5 pt (the raw `- ` is narrower than the 24 pt indent),
  and a quote right by about 9 pt (the bar goes and the raw `> ` takes its place).
- **It changes the page at rest, not just on reveal.** With it on, list markers hang too, so list
  text sits flush with body text and the bullets live in the margin. That is iA Writer's look and
  some will prefer it, but it is a design change to every list, not only a fix for reveal jitter.
  The task's instruction was to default to off unless the evaluation clearly said otherwise; the
  reveal benefit is clear, the at-rest change is a matter of taste, so it stays off.
- **It needs margin.** A hanging prefix sits left of the column, inside `space.page.side`. At the
  default column width there is plenty of room; in a window narrow enough that the column fills
  it, a long prefix (`######`, `1000.`) is clamped at the window edge and can crowd the text.
- **Cost is nil.** The hung prefix is one extra small layout per revealed line, cached like any
  other; nothing on the keystroke path changes.

Element-level reveal inside a line (bold, links) is unaffected either way: the rest of the line
still shifts to make room, as the stack design specifies.

## Screenshots

There is no in-app dump facility yet (`<ENV_PREFIX>DUMP` does not exist in `crates/` or `xtask/`),
and the macOS shell cannot run under the sandbox, so these are headless frames from the renderer
itself: `DocView` over the test fonts (Noto Sans and Noto Sans Mono, regular only), 800 by 600 pt
at scale 2. Regenerate them with

```text
cargo test -p editor-render --test frame_dumps -- --ignored --nocapture
```

which writes them to `$TMPDIR/frame-dumps/` (on the build machine, `/tmp/claude-501/frame-dumps/`).
The files are not committed.

| Cursor on | Off (before) | On, 32 pt gutter (after) |
|---|---|---|
| a paragraph (nothing revealed) | `hang-off-none.png` | `hang-on-none.png` |
| the `##` heading | `hang-off-heading.png` | `hang-on-heading.png` |
| the first list item | `hang-off-list.png` | `hang-on-list.png` |
| the quote | `hang-off-quote.png` | `hang-on-quote.png` |

What they show:

- `hang-off-heading.png`: `## ` appears in the column and "Reading list" moves right.
  `hang-on-heading.png`: `##` sits in the margin and "Reading list" stays at the column edge.
- `hang-off-list.png`: the bullet becomes `- ` and the item's text moves left from the indent.
  `hang-on-list.png`: the bullet becomes `-` in the same margin spot and the text does not move.
- `hang-off-quote.png`: the quote bar disappears and the text moves right behind `> `.
  `hang-on-quote.png`: the bar stays, `>` appears in the margin, and the text does not move.
- `hang-on-none.png` against `hang-off-none.png`: the at-rest difference, list text flush with the
  body text.

## How it works

- `size.hanging_prefix` above 0 turns it on. `space.prefix.gap` separates a hanging prefix from the
  text; prefixes are right-aligned against the column's left edge.
- List markers (bullet, number, task box) draw in the margin, and the item's text starts at the
  column edge.
- When the cursor reveals a heading, quote or list prefix, the raw prefix is taken out of the
  line's text and laid out on its own in the margin, in `color.syntax.reveal`. A revealed quote
  keeps its bars and its text offset. The caret can sit inside the hung prefix.
- Off (the default), list markers sit in the indent, which grows to fit a wide number (`1000.`)
  plus `space.prefix.gap`, and revealed prefixes are part of the line as before.
