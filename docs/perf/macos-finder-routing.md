# macOS Finder routing

The stack doc leaves one question open: when the app is already running, which instance does
Launch Services hand a Finder or `open` request to? The window processes are spawned directly
(`posix_spawn` from the launcher or from another window), not through Launch Services, so the
answer is not obvious. The design makes the answer not matter, and this test checks both the
design and the answer.

## How routing works

- Every instance's `application:openURLs:` handler spawns one sibling window process per URL:
  the bundle executable with `-- <path>`, `posix_spawn` with `POSIX_SPAWN_SETSID`, stdio on
  `/dev/null`, no inherited descriptors, and `GDK_DISABLE`, `GDK_DEBUG`, `GSK_RENDERER` and every
  `<P>*` variable stripped. It never opens a URL in itself.
- The spawning instance yields activation to the child (`yieldActivationToApplication:`) once
  the child's `NSRunningApplication` can be found by pid, looking for up to 500 ms. Best effort.
- An instance started with no path is the router. It creates no window, waits up to 2 s from
  `applicationDidFinishLaunching:` for `openURLs`, and exits 0 once 500 ms have passed since its
  last spawn.
- AppKit is told not to treat command-line arguments as documents
  (`NSTreatUnknownArgumentsAsOpen` registered as `NO`), and a windowed instance ignores an open
  request for one of its own arguments that arrives before it finishes launching. Either guard
  alone stops a window from spawning a copy of itself.

## What is logged

With `<P>MARKS` set to a log path, each instance that receives `openURLs` appends one line,
whose value is that instance's pid:

| Line | Meaning |
|---|---|
| `router_open_urls,<pid>` | a router (no path, no window) received the request |
| `window_open_urls,<pid>` | an instance with a window received the request |
| `routed_spawn,<pid>` | the pid of the sibling window process just spawned |

Children never inherit `<P>MARKS` (it is stripped), so every line comes from an instance that
Launch Services or the launcher started. Launch Services does not pass the shell's environment
to an app it starts, so the commands below hand `<P>MARKS` over with `open --env`.

## Procedure

Run on the Mac with the Bash sandbox off (the sandbox blocks Launch Services and the
WindowServer). `<BUNDLE_NAME>`, `<SLUG>` and `<P>` are the bundle name, slug and env prefix from
`app_meta`. `$LOG` is a fresh marks log and `$D` a directory holding `a.md`, `b.md` and `c.md`.

1. Build and register the bundle:

   ```
   cargo xtask bundle-mac --release
   /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f target/dist/<BUNDLE_NAME>.app
   ```

2. With nothing running:

   ```
   open -a target/dist/<BUNDLE_NAME>.app --env <P>MARKS=$LOG $D/a.md
   sleep 3; pgrep -lf <BUNDLE_NAME>; cat $LOG
   ```

   Expected: one window process holding `a.md`; the router has exited (it is not in `pgrep`);
   the log has one `router_open_urls` line and one `routed_spawn` line.

3. With `a.md`'s window open from the launcher, then two `open` requests:

   ```
   : > $LOG
   <P>MARKS=$LOG target/dist/<BUNDLE_NAME>.app/Contents/MacOS/<SLUG> -b $D/a.md
   open --env <P>MARKS=$LOG $D/b.md
   open --env <P>MARKS=$LOG $D/c.md
   sleep 3; pgrep -lf <BUNDLE_NAME>; cat $LOG
   ```

   Expected: two new processes (`b.md` and `c.md`), `a.md`'s window untouched, and no process
   holding two files. Record which instance received each `openURLs`: a `window_open_urls` line
   with `a.md`'s pid means Launch Services picked the running window; a `router_open_urls` line
   means it started a router.

4. In Finder, select two files and double-click (or press Cmd+Down). Expected: two processes.

## Results

| Step | Date | Machine | Result | Instance that received `openURLs` |
|---|---|---|---|---|
| 2 | | | | |
| 3 (`b.md`) | | | | |
| 3 (`c.md`) | | | | |
| 4 | | | | |
