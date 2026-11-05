# Manual checks

These are the hand checks to run before a release. Work through them on the Mac, and on Linux where a section says so.

These checks need a real window, so the automated suite can't run them. They are pending for
v0.9. Run them on the Mac (and on Linux at Checkpoint 3, Task 45), then tick them off.

Build the release bundle and open a file with the window binary inside it:

```sh
cargo xtask bundle-mac --release
target/dist/<BundleName>.app/Contents/MacOS/<name> test.md
```

Starting it this way waits until the window closes, the same as the launcher does without `-b`.

## Test file

Put this in `test.md`, and create an `other.md` and a `run.sh` next to it:

```markdown
# Heading with **bold** text

Plain line with **bold** and [web](https://example.com) and [other](other.md) and [script](run.sh).

- [ ] task one
- [x] task two

![pic](missing.png)

word_one   word_two, punct!
```

## Mouse (Task 38b)

1. **Click places the cursor.** In Normal, click a letter of "Plain". The block cursor lands on
   that letter, even when you click its right half. Press `i` and click between two letters:
   the bar lands on the nearer boundary.
2. **Reveal waits for release.** Press and hold on "bold". The `**` markers do not appear while
   the button is down, and do appear when you release. Drag sideways from "bold": the text does
   not shift under the pointer.
3. **Drag enters Visual.** Drag across "Plain line" in Normal. The status shows VISUAL and the
   selection follows the pointer. `d` deletes it and `u` undoes that.
4. **Drag from Insert.** Press `i`, type `xy`, then drag across some text. Visual appears. Press
   Esc, then `u`: the `xy` goes as one step.
5. **Double-click.**
   - On `word_two`: the word is selected in Visual, with the cursor on its last `o`.
   - On the run of spaces between the words: the whole run is selected.
   - On `,`: just the comma is selected.
   - On the `_` inside `word_one`: the whole word is selected.
6. **A click in Visual leaves it.** Select with `vll`, then click elsewhere: you're back in
   Normal at the click. Do it again, clicking a checkbox or a link this time: Visual still ends.
7. **Task checkbox.** Click the box of "task one". It becomes `[x]`, the cursor and the view stay
   put, and `u` undoes it. Repeat in Insert. In reader view (Cmd+Shift+R), nothing changes.
8. **Links.**
   - In Normal, a plain click on "web" opens the browser, and the cursor doesn't move.
   - A double-click on "web" opens it once, not twice.
   - In Insert, a plain click on "web" only places the caret. Cmd+click opens it.
   - A plain click on "other" opens `other.md` in a new window of ours.
   - A plain click on "script" asks `open /.../run.sh? [o]pen [c]ancel`, and `c` cancels it.
     While that question shows, clicks and drags do nothing, and the wheel still scrolls.
   - Press on "web", drag off the text and release: nothing opens.
   - In reader view (Cmd+Shift+R), a plain click on "web" opens it.
9. **Waiting launcher.** From a terminal, start the window without `-b`, then click "other". A
   new window opens with `other.md`, and the terminal is still waiting. It returns only when
   the first window closes.
10. **No clicks under a question.** Type `:` (or `/`, or Cmd+L for the `link:` prompt), then
    click and drag in the text: nothing moves. Run `:s/o/0/gc` and click while it asks: nothing
    happens. Press Esc to get out.
11. **Wheel and trackpad.**
    - Trackpad scrolling is smooth and the cursor stays put.
    - A mouse wheel scrolls about one line per notch.
    - Typing or moving afterwards scrolls back to the cursor.
12. **Table picker.**
    - On an empty line, run `:table`. The highlighted size follows the pointer, and clicking a
      cell inserts a table of that size.
    - Run `:table` again and scroll until the cursor's line leaves the top: the picker stays at
      the top of the window.
    - Make the window very narrow: the picker stays inside it.
13. **Long open-link question.** Click a link to
    `ftp://host/a/very/long/path/that/goes/on/and/on/and/on/and/on/and/on/report.pdf`. The status
    bar shows the start, `…`, `report.pdf` and the answers on one row, and nothing overlaps the
    line:col on the right.
14. **Hidden characters.** Click a link whose target holds a real U+202E (right-to-left
    override). The question shows `\u{202e}` literally, not reversed text.
15. **Composition.** With a Japanese input method, start composing in Insert, then click
    elsewhere on the same line. The composition is dropped, not committed at the click.
16. **Vim off** (needs Task 35's config: `vim = false`). Dragging selects with no mode shown,
    double-click selects the word, and a plain click on a link opens it.

## Editing and file state (Tasks 32, 33b, 39b, 41, 43)

17. **Vim off keys (Task 41)**, with `vim = false`:
    - Cmd+F opens the find bar. Enter finds the next match and Shift+Enter the previous one.
    - Alt+Left and Alt+Right move by word. Cmd+Left and Cmd+Right go to the line start and end.
      Cmd+Up and Cmd+Down go to the document start and end.
    - Cmd+B bolds and Cmd+I italicises the selection. Cmd+Z undoes and Cmd+Shift+Z redoes.
18. **Link prompt (Task 39b).** Select a word and press Cmd+L. A `link: ` prompt appears. Type a
    URL and press Enter: the word becomes `[word](url)`. With nothing selected, Cmd+L then Enter
    puts in `[](url)` with the cursor inside the `[]`. Esc cancels and leaves everything as it was.
19. **Same file in two windows (Task 33b).** Open `test.md`, then open it again from a second
    terminal. The second window asks
    `already open in another window (pid N): [o]pen read-only [e]dit anyway [q]uit`. Esc does
    nothing there. `o` opens it read-only, `e` edits anyway, and `q` closes the window.
20. **Changed on disk (Task 32).**
    - While the window is clean, edit `test.md` in another editor and save. The window reloads.
    - Make the window dirty, then change the file elsewhere. The window warns instead of
      reloading, and `:w` asks before writing over the other change.
    - `rm test.md`: the window warns that the file is gone.
    - `chmod 444 test.md`, then focus the window: it shows read-only. `chmod 644 test.md`, then
      focus it again: the read-only warning clears.
21. **Title and edited dot (Task 43).** The window title shows the file name. Type something:
    the edited dot appears in the red close button (on Linux, a mark in front of the title).
    `:w` clears it.

## Config and theme (Task 35)

The config file is `${XDG_CONFIG_HOME:-$HOME/.config}/<name>/config.toml`. Steps 16 and 17
need `vim = false` in it.

22. **Live dark mode on macOS.** Remove any config file. Open a markdown file with a heading, a link, a fenced code block and a table. With the window open, switch System Settings > Appearance between Light and Dark. Expect the whole window to repaint in the new theme within a second: a charcoal page with light text in dark, the usual paper white in light, with no stale light or dark lines, patches or status bar.
23. **Overlays follow the theme.** In dark, type `:help` and Enter: the help panel is the dark overlay colour. Press Esc, type `:table` and Enter: the grid picker is in dark overlay colours. Press Esc. Open `:help` again and toggle the system appearance while it's up: the panel repaints in the new theme and stays open.
24. **Resizing in dark.** In dark, drag a window edge to resize. Expect the strip revealed during the drag to be dark, not white.
25. **Theme overrides.** Write this config file:
    ```toml
    [tokens]
    "color.caret" = "#E06C00"

    [tokens.dark]
    "color.bg" = "#000000"
    "color.link" = "#FF5F87"
    ```
    Open a file with a link while the system is Light. Expect the normal light page, an orange caret and the usual blue link. Switch the system to Dark with the window open. Expect a pure black page, pink links and the caret still orange. Switch back: the light defaults return.
26. **A fixed theme.** Add `theme = "dark"` at the top of that file and reopen. Expect dark from the first frame whatever the system says, and toggling the system appearance changes nothing. Remove the line afterwards.
27. **Bad TOML.** Replace the config file with `vim = = false` and open a file. Expect the editor to open normally with default settings (vim keys, NORMAL shown) and a status bar warning starting `config error: ` with the parser's message, fading after the usual time.
28. **Unknown keys and bad values.** Replace the config file with:
    ```toml
    wrap = true
    column_width = -3
    [tokens]
    "color.nope" = "#000000"
    "font.size" = 0
    ```
    Open a file. Expect it to open normally at the default column width and text size, with one status bar warning: `unknown setting: wrap; bad value for column_width: -3; unknown setting: tokens.color.nope; bad value for tokens.font.size: 0`. Delete the config file afterwards.
29. **Linux desktop (GNOME or KDE).** Open a file and toggle the desktop's dark style. Expect a repaint in the new theme within a second. Close and reopen: the first frame is already in the last scheme seen, with no flash of the other theme (the scheme is kept in `${XDG_STATE_HOME:-$HOME/.local/state}/<name>/appearance`). With no portal (a bare container) the editor opens light with no message and no delay.
30. **Bench runs stay light.** With the system in Dark, run a bench start (the usual `cargo xtask bench` run). Expect the bench window to paint light, so its numbers compare with earlier runs.

## Linux (Task 45)

Repeat 1, 2, 3, 5, 7, 8 (Ctrl+click in Insert), 11 and 12 under GTK, and 29. On Linux a wheel notch
scrolls about the page height to the power 2/3, in points, as GTK's own scrolled windows do.

## Checkpoint 3 (Task 45)

Paste each results directory the bench prints back into the session.

31. **macOS typing run.** Close other heavy apps and keep the screen unlocked, then run
    `cargo xtask bench type --manual --keys 500`.
    Type ordinary prose, but switch modes often: Esc and `i`, `o` for new lines, a few `:w`
    saves, and `u` now and then. Checkpoint 2 missed once on a stall after switching modes, so
    the switches matter here. Expect key_ms p95 at or under 16 ms.
    *Done 2026-11-05: p95 12.4 ms, PASS (light on mode switches; lean on them in step 32).*
32. **Linux laptop run.** On the reference laptop (Wayland session, `wtype` installed), from the
    repository root, run `scripts/checkpoint-linux.sh 3` and paste back the directories it
    prints.
33. **Input methods on Linux.** On Hyprland, type with dead keys (for example `'` then `e`) and
    with a compose sequence. Expect the composed character, once, at the cursor. Repeat on one
    compositor without text-input-v3 (your pick).
34. **Side by side with neovim.** In the same session, type the same text into this editor and
    into neovim in foot, and record both (a 240 fps camera works on both platforms). There's no
    target: note how they compare.
35. **Mouse and links on Linux.** Run steps 1, 2, 3, 5, 7, 8 (Ctrl+click in Insert), 11, 12 and
    29 above under GTK.
36. **X11 smoke run (best effort).** Under XWayland, run `GDK_BACKEND=x11 <name>
    target/fixture/test.md`, type a few lines, `:w`, then close. Expect it to open, type, save
    and close the same as under Wayland.
37. **R&D record.** Closing the branch's R&D record (`wombat-rnd`) is your call once the
    checkpoint is judged.

## Fonts on macOS (Task 46)

The config file is `${XDG_CONFIG_HOME:-$HOME/.config}/<name>/config.toml`. The font cache is
`${XDG_CACHE_HOME:-$HOME/.cache}/<name>/font-paths`.

38. **Configured families.** Write this config file:
    ```toml
    [font]
    body = "Georgia"
    mono = "Menlo"
    ```
    (`font.body = "Georgia"` and `font.mono = "Menlo"` as dotted keys at the top of the file
    work the same.) Open a markdown file with a paragraph holding `*italic*` and `**bold**`
    words, a heading, some `inline code` and a fenced code block. Expect body text and the
    heading in Georgia (a serif), italics in true Georgia Italic (not a slanted sans), bold in
    Georgia Bold, and code in Menlo. No status bar message.
39. **The cache remembers them.** Close and reopen the same file. Expect the same faces. The
    cache file's first line is `font-paths v2 macos`, and it holds `key	name	Georgia`,
    `key	name	Menlo` and a `sans` line ending `Georgia.ttf`.
40. **Changing a family.** With the window open, change `body` to `"Helvetica Neue"`. Expect no
    change in the open window (families are taken at open). Close and reopen: body text is now
    Helvetica Neue, code still Menlo.
41. **Unknown families.** Change the table to `body = "No Such Face"` and `mono = "Nor This
    One"`. Open a file. Expect the usual system faces (SF for text, SF Mono for code), exactly as
    with no config, and no status bar message.
42. **Defaults unchanged.** Delete the config file and reopen. Expect SF and SF Mono as before
    this task, with italics still in SF Italic.
