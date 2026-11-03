# Launcher versus GUI process models, and fork-without-exec safety with GLib/GTK

Scope: how GUI editors and terminals split (or do not split) the command-line launcher from the GUI process, how they wait, detach and report exit codes, and whether foo_edit's proposed "single binary, fork without exec before any GLib/GTK init on Linux, fork+exec on macOS" launcher is safe. Notation: each finding ends with its source in brackets. Items marked "from source read" were checked directly in the upstream code on 2026-11-03.

## 1. Prior art: launcher vs GUI, waiting, detaching, instancing, exit codes

### Takeaway
There are three families. (a) Self-forking single binaries (gVim, Neovide, kitty) where the shell command and the GUI are the same executable and a flag picks wait or detach. (b) Thin client plus long-lived server or single instance (Zed, VS Code, Sublime, emacsclient, footclient, kitty `--single-instance`, `alacritty msg`) where "wait" means "wait for a message from the server", which is the source of the wrong-window and early-return bugs. (c) Separate launcher binary that execs a GUI binary (WezTerm). foo_edit's "one process per file, launcher waits on its own child" is closest to (a), and kitty's C launcher on Linux is almost exactly the proposed design.

### Cited Findings

**gVim**
- gVim forks and the original process exits when the GUI starts, so the shell regains the prompt; for "a mail program that waits for gvim to exit" you must use `gvim -f`, `vim -gf` or `:gui -f`; `--nofork` is the same as `-f` ([vim gui_x11.txt, *gui-fork*](https://raw.githubusercontent.com/vim/vim/master/runtime/doc/gui_x11.txt)).
- Vim will not fork when it has running jobs "because the processes would no longer be child processes" ([vim gui_x11.txt](https://raw.githubusercontent.com/vim/vim/master/runtime/doc/gui_x11.txt)).
- The footgun: when an editor detaches, git sees the editor exit immediately, reads an empty message and aborts. The same symptom is reported for VS Code without `--wait` ("git commit aborts before I can create a commit message") ([dev.to](https://dev.to/meg_gutshall/git-commit-aborts-before-i-can-create-a-commit-message-and-i-use---wait-1n8n)).

**Neovide**
- PR #2512 "Don't fork by default" changed the default: "previously Neovide always forked"; it also never forks when no tty is attached; forking is opt-in with `--fork` or `NEOVIDE_FORK=1`. Shipped in 0.13.0. Maintainer reasoning: "programs that fork are the exception, not the rule" ([neovide PR #2512](https://github.com/neovide/neovide/pull/2512)).
- Linked issues include #2579 (always attached to the terminal), #2597 (cursor locked in the CLI after launch), #2330 (not focused when launched from a terminal) ([neovide PR #2512](https://github.com/neovide/neovide/pull/2512)).
- A user found that redirecting output (`> /dev/null 2>&1`) defeated the tty check so the fork silently did not happen ([neovide PR #2512](https://github.com/neovide/neovide/pull/2512)).
- Current docs: `--fork`: "Detach from the terminal instead of waiting for the Neovide process to terminate. This parameter has no effect when launching from a GUI." ([neovide.dev command-line reference](https://neovide.dev/command-line-reference.html)).
- Implementation (from source read): `maybe_disown` calls `fork::daemon(true, false)` (fork plus setsid), the parent exits 0, and the daemonised child then runs `Command::new(current_exe)` with stdio nulled, args forwarded and an env marker, `spawn()`s it and exits. So Neovide's detach is fork + setsid + a full re-exec of itself, paying startup twice, with exit status discarded ([neovide src/main.rs](https://raw.githubusercontent.com/neovide/neovide/main/src/main.rs)).

**kitty** (most relevant prior art)
- `--detach`: "Detach from the controlling terminal, if any. On macOS use `open -a kitty.app -n` instead." `--single-instance` makes new invocations open a window in the existing process; `--instance-group` partitions that; `--wait-for-single-instance-window-close` keeps the client alive until its window closes ([kitty invocation docs](https://sw.kovidgoyal.net/kitty/invocation/)).
- Implementation (from source read, `kitty/launcher/main.c`): the C launcher, before Python, GLFW or OpenGL are loaded, redirects stdio (stdin to /dev/null, stdout/stderr to `--detached-log` or /dev/null), creates a pipe, `fork()`s; the parent blocks on the pipe "until child has done setsid() before exiting so that it doesnt get a SIGHUP" and then exits 0; the child calls `setsid()`, closes the pipe and simply carries on in-process to become kitty. On Linux there is no exec ([kitty launcher main.c](https://raw.githubusercontent.com/kovidgoyal/kitty/master/kitty/launcher/main.c)).
- macOS exception in the same code: "fork() without exec() is unsafe on macOS. It used to work since in this case we havent yet loaded any major Cocoa libraries but in Tahoe 26.2 Apple broke that as well. So now just exec() on --detach", using a `KITTY_EXEC_FOR_DETACH=<pid>` env marker to stop the re-exec'd child detaching again ([kitty launcher main.c](https://raw.githubusercontent.com/kovidgoyal/kitty/master/kitty/launcher/main.c)).
- Issue #8680: `kitty --detach` run under a pty that then closed (e.g. `script -c "kitty --detach"`) failed about 1 in 30 times in 0.42.0. strace showed a SIGHUP race: the parent exits, the pty closes, and SIGHUP reaches the child before it has called `setsid()`. Fix: make the parent wait on the pipe (a zero-byte read was not enough, changed to a non-zero read). Kovid rejected double fork: "forking is expensive, there's no actual need for a double fork here." ([kitty issue #8680](https://github.com/kovidgoyal/kitty/issues/8680)).
- kitty's Python side has a `safer_fork()` that reseeds OpenSSL's RNG in the parent and clears `atexit` handlers in the child, showing the kind of state that must be fixed up after a fork that does not exec ([kitty utils.py](https://raw.githubusercontent.com/kovidgoyal/kitty/master/kitty/utils.py)).

**Zed**
- From source read (`crates/cli/src/main.rs`): the `zed` CLI is a separate binary. Each invocation opens a one-shot IPC server (`zed-cli://{server_name}`), sends `CliRequest::Open { paths, wait, open_behavior, ... }`, then receives `Stdout`, `Stderr` and `Exit { status }` messages and finally `std::process::exit(exit_status)` ([zed cli main.rs](https://raw.githubusercontent.com/zed-industries/zed/main/crates/cli/src/main.rs)).
- On Linux it first tries a datagram socket `zed-{channel}.sock` (existing instance); if absent, `boot_background` forks, the child calls `setsid()`, closes fds and `execvp`s the app. On macOS it uses Launch Services (`LSOpenFromURLSpec`); on Windows a named mutex plus named pipe. A `--foreground` mode runs the app directly and blocks on `cmd.status()` ([zed cli main.rs](https://raw.githubusercontent.com/zed-industries/zed/main/crates/cli/src/main.rs)).
- So Zed is single-instance with a cli-to-app protocol; `--wait` is a protocol flag and the exit code is whatever the app chooses to send.

**VS Code**
- `code --wait` is a client of a single-instance app. Issue #170444: closing one pane of a split editor counts as "done", so `--wait` returns and git commits while the message is still open in another pane ([vscode #170444](https://github.com/microsoft/vscode/issues/170444)).
- Issue #157961: `--wait` returns no meaningful exit code; a merge tool returned success almost immediately and SourceTree deleted the merge files ([vscode #157961](https://github.com/microsoft/vscode/issues/157961)).

**WezTerm**
- From source read: the `wezterm` binary handles `cli`, `imgcat`, `record` itself and for `start`, `ssh`, `connect`, `serial`, `ls-fonts`, `show-keys` delegates to the sibling `wezterm-gui`. On Unix it closes stray fds and `exec`s (process replacement, no fork); on Windows it spawns, waits and exits with the child's code (with `--attach-parent-console`) ([wezterm src/main.rs](https://raw.githubusercontent.com/wezterm/wezterm/main/wezterm/src/main.rs)).

**foot**
- `foot --server` hosts many windows in one process to share fonts and glyph caches; supports systemd socket activation (`foot-server.{service,socket}`) ([foot.1](https://codeberg.org/dnkl/foot/raw/branch/master/doc/foot.1.scd)).
- `footclient` does not exit until its terminal has terminated and exits with the client application's code; `-N/--no-wait` exits 0 once the server has been told to open a window; reserved codes 220 (footclient failure) and 230 (server failure) ([footclient.1](https://codeberg.org/dnkl/foot/raw/branch/master/doc/footclient.1.scd)). This is a clean precedent for reserving launcher-side exit codes distinct from the payload's.

**Alacritty**
- `alacritty msg create-window` asks a running instance over a socket to open a window in the same process; `alacritty --daemon` starts without an initial window ([alacritty-msg.1](https://raw.githubusercontent.com/alacritty/alacritty/master/extra/man/alacritty-msg.1.scd); [alacritty.1](https://raw.githubusercontent.com/alacritty/alacritty/master/extra/man/alacritty.1.scd)).

### Inferences
- Every client/server design (Zed, VS Code, footclient, kitty single-instance, emacsclient) has to invent a "window closed, here is the code" message, and VS Code shows how that goes wrong. foo_edit's one-process-per-file model sidesteps it entirely: the launcher's waitpid on its own child is the ground truth, and crashes show up as WIFSIGNALED for free.
- Wait-by-default with an explicit detach flag is now the mainstream choice (Neovide reversed its default in 0.13; footclient waits by default; gVim's fork-by-default is the historic footgun). foo_edit's "wait by default, `-b` detaches" matches current consensus.
- kitty's Linux `--detach` is direct prior art for fork without exec in a GUI program, done before any toolkit initialisation, in production for years. Its #8680 race applies to foo_edit's `-b` mode exactly.

### Gaps
- Sublime Text `subl -w`, Kate (`kate -b`/`--block`, single instance via D-Bus), gnome-text-editor/gedit (GApplication single instance; `--standalone`/`-s`), Lapce, Helix GUIs and `emacsclient -c` were not source-checked in this pass. From prior knowledge (unverified here): `subl -w` and `kate -b` block until the file's tab closes via the running instance; GApplication editors are single-instance by default; Lapce detaches by default with a `--wait` flag. Treat as hypotheses.
- I did not locate a specific Neovide issue titled along the lines of "GUI dies when terminal closes"; the closest linked item is #2579.

## 2. Is fork() without exec safe before GLib/GTK initialisation in Rust?

### Takeaway
GLib's maintainers say you cannot fork without exec and keep using GLib in the child once GLib is running, because of its worker threads. That hazard does not apply if the fork happens while the process is still single-threaded and no GLib/GTK API has been called: the child is then indistinguishable from a freshly started process that has run library constructors. kitty does exactly this on Linux. The real risks are narrower: threads started by library constructors, Rust std state, signal races, and stdio inherited from the terminal.

### Cited Findings
- Colin Walters (GLib): "You basically can't do a fork() without exec() in a GLib program, and call GLib API in the child after the fork. We depend on the worker thread for various functionality, and calling fork() kills all other threads." Dan Winship floated a `pthread_atfork()` mitigation; Walters: "the complexity makes my head hurt." Bug closed as duplicate, not fixed ([GNOME bug 698606](https://bugzilla.gnome.org/show_bug.cgi?id=698606)).
- GLib's `g_spawn` child-setup function runs between fork and exec and may only use async-signal-safe functions ([GLib spawning docs](https://web.mit.edu/barnowl/share/gtk-doc/html/glib/glib-Spawning-Processes.html)).
- gtk-devel-list (2016): forked children hung on glibc and musl due to libc-internal locks (e.g. in `rand()`), when forking in a threaded process ([gtk-devel-list, June 2016](https://lists.gnome.org/archives/gtk-devel-list/2016-June/msg00004.html)).
- GLib's ELF constructor (`glib_init_ctor` -> `glib_init`) only runs `g_messages_prefixed_init`, `g_debug_init`, `g_quark_init`, `g_error_init`; it starts no threads on non-Windows (from source read) ([glib/glib-init.c](https://raw.githubusercontent.com/GNOME/glib/main/glib/glib-init.c)). The GLib worker thread is created lazily by `g_get_worker_context()` ([glib/gmain.c](https://raw.githubusercontent.com/GNOME/glib/main/glib/gmain.c)).
- nix `unistd::fork` safety: "In a multithreaded program, only async-signal-safe functions like `pause` and `_exit` may be called by the child (the parent isn't restricted) until a call of `execve(2)`. Note that memory allocation may not be async-signal-safe and thus must be prevented." ([nix fork docs](https://docs.rs/nix/latest/nix/unistd/fn.fork.html)).
- Rust `CommandExt::pre_exec`: after fork, "normal operations like `malloc`, accessing environment variables through `std::env` or acquiring a mutex are not guaranteed to work (due to other threads perhaps still running when the `fork` was run)"; all fds and mappings are duplicated and "It is your responsibility to make sure that the closure does not violate library invariants" ([Rust CommandExt docs](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html)).
- kitty forks without exec on Linux before loading its GUI stack and continues in the child; it had to switch macOS to exec because "in Tahoe 26.2 Apple broke that as well" ([kitty launcher main.c](https://raw.githubusercontent.com/kovidgoyal/kitty/master/kitty/launcher/main.c)).
- macOS: Apple engineers state CoreFoundation "cannot be used on the child-side of fork(). If you fork(), you must follow that with an exec*() call", and this includes `daemon()` because its internals may touch CF; a double-fork daemon hit the warning on its first `XOpenDisplay()` ([Apple darwin-dev, 2009](https://lists.apple.com/archives/darwin-dev/2009/May/msg00019.html); [Apple xcode-users, 2008](https://lists.apple.com/archives/xcode-users/2008/Apr/msg00178.html)).
- kitty's `safer_fork()` reseeds the SSL RNG and clears atexit handlers, illustrating state duplicated by a non-exec fork ([kitty utils.py](https://raw.githubusercontent.com/kovidgoyal/kitty/master/kitty/utils.py)).

### Inferences
- Key point for the latency argument: if the single binary links GTK, the dynamic loader maps and relocates all ~113 DT_NEEDED libraries and runs their constructors *before* `main`. The fork-without-exec design therefore pays the load once (in the launcher, before main), and the child inherits the mappings copy-on-write. Time to window is about the same as "2.8 ms libc launcher + exec of GTK binary"; the difference is that the launcher itself is no longer 2.8 ms to main, which only matters if the launcher has work to do that is not followed by a window (e.g. `--help`, `--version`, error paths). Neovide's model (fork + exec self) is what produced the measured double cost.
- Safety rests on a single precondition: exactly one thread and no GLib/GTK/GIO call before fork. Library constructors are the unverifiable part (113 libs). GLib's own constructor is clean. Rather than audit, assert it at runtime: read `/proc/self/status` `Threads:` (or count `/proc/self/task`) at the top of `main`; if it is not 1, fall back to fork+exec. This is cheap and turns an assumption into a check.
- Rust std starts no threads before `main` in a normal binary (the test harness does, so the launcher path must not be exercised in-process from `cargo test`; spawn the binary instead). Avoid anything before fork that could start threads: no `tokio`/`rayon` init, no `env_logger` with background writers, no GTK/GIO calls (including `gio::Settings`, `glib::set_application_name`, `gtk::init`).
- In the child after a single-threaded fork, malloc, env access and mutexes are fine (the async-signal-safe restriction is about forks taken while other threads exist). The child can call `gtk::init` normally.
- Rust's `print!/eprintln!` panic if writing fails. A detached child that still holds the terminal as stderr will get EIO after the terminal closes, so `eprintln!` in the window process can panic. Redirect the child's stdio (stdin to /dev/null; stdout/stderr to /dev/null or a log file) as kitty does. Rust ignores SIGPIPE by default, and that disposition is inherited identically by fork and exec, so it is not a difference between the two designs.
- On macOS the fork+exec path is mandatory, and Neovide/kitty both confirm it; kitty's env-marker trick is a tidy way for the re-exec'd child to know it is already detached.

### Gaps
- No GNOME statement was found that explicitly blesses "fork before any GLib call"; the safety argument is inference from the maintainers' stated reason (threads) plus kitty's production use. No reported problems with fork-before-gtk_init were found, but absence of evidence is weak.
- Not verified which of GTK4's ~113 dependencies run constructors, or whether any start threads (Mesa/GL loaders are normally loaded later via dlopen, but this was not checked). The runtime thread-count assertion covers this.

## 3. Alternatives: posix_spawn, vfork/clone, separate launcher vs multi-call binary, deferring library load

### Takeaway
`posix_spawn` with `POSIX_SPAWN_SETSID` (glibc 2.26+) is the clean "spawn a detached child" primitive, but it always execs, so it only helps the two-binary design. The fastest correct options are either kitty-style fork-without-exec in one binary, or WezTerm-style separate launcher that execs/spawns a GUI binary. A busybox-style multi-call binary does not help because it still loads GTK on every invocation. Deferring GTK with dlopen is possible in principle but impractical with gtk-rs.

### Cited Findings
- `POSIX_SPAWN_SETSID` exists "since glibc 2.26", is GNU-only (needs `_GNU_SOURCE`), value 0x80, added for BZ 21340 by Adhemerval Zanella, with a glibc test checking the child's session id ([man-pages commit](https://git.tldp.org/LDP/man-pages/commits/commit/bf9cc31523d7c9b3c7d0a15e1e4e9c105a7ff7ac); [glibc-cvs 2017-q2](https://sourceware.org/ml/glibc-cvs/2017-q2/msg00090.html); [posix_spawn(3)](https://manpath.be/f37/3/posix_spawn); [tst-posix_spawn-setsid.c](https://codebrowser.dev/glibc/glibc/posix/tst-posix_spawn-setsid.c.html)).
- Rust `Command` exposes `process_group` ("Equivalent to a `setpgid` call in the child process, but may be more efficient") but no setsid; setsid needs `pre_exec` (which disables the posix_spawn fast path) or calling `posix_spawn` directly via `libc` ([Rust CommandExt docs](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html)).
- WezTerm ships `wezterm` (CLI) and `wezterm-gui` as separate executables, with the launcher exec'ing the GUI ([wezterm src/main.rs](https://raw.githubusercontent.com/wezterm/wezterm/main/wezterm/src/main.rs)).
- Zed on Linux uses fork + setsid + close fds + execvp from its cli binary ([zed cli main.rs](https://raw.githubusercontent.com/zed-industries/zed/main/crates/cli/src/main.rs)).
- kitty prefers a single fork over double fork: "forking is expensive, there's no actual need for a double fork here" ([kitty issue #8680](https://github.com/kovidgoyal/kitty/issues/8680)).

### Inferences
- Multi-call binary: if one ELF has GTK in DT_NEEDED, every argv[0] personality pays the ~43 ms load before main. It only helps if the launcher personality is a different ELF, which is just the two-binary design again.
- dlopen-deferred GTK: glibc has no lazy loading of DT_NEEDED (lazy binding defers symbol resolution, not library loading). Making GTK lazy would mean not linking it, i.e. building the GUI as a `cdylib` that the launcher `dlopen`s in the child. That works technically (and gives one ELF the user sees plus a .so), but adds packaging complexity and rpath issues and loses nothing over a second executable. Not recommended.
- Static linking GTK4 is not a supported configuration (GTK relies on loadable modules: GIO modules, media backends, input methods, GL drivers), so it is not a route to cutting load time. This is prior knowledge, not sourced here.
- vfork/clone(CLONE_VFORK) only help when the child immediately execs; they buy nothing for fork-without-exec, and with exec they are what `posix_spawn` already uses internally on modern glibc.
- Double fork is unnecessary: a setsid'd child never reacquires a controlling terminal unless it opens a tty without `O_NOCTTY`, which GTK does not do. Double fork would also stop the launcher from waiting on its grandchild.

### Gaps
- Did not measure fork() cost for a process with ~113 libraries mapped (page-table copy). Expected to be sub-millisecond to a few ms; worth timing in the spike.
- Did not source a GTK maintainer statement on static linking.

## 4. Waiting on a child in another session; SIGINT/SIGHUP; crash detection; pidfd

### Takeaway
`setsid()` changes session and process group, not parentage, so the launcher can `waitpid` the window as usual and gets exit status or terminating signal directly. The traps are the window of time between fork and the child's setsid (Ctrl+C or SIGHUP can kill the child) and the launcher's own signal handling.

### Cited Findings
- kitty #8680: if the parent exits and the pty closes before the child has called `setsid()`, SIGHUP reaches and kills the child; fixed by having the parent block on a pipe until the child has run setsid ([kitty issue #8680](https://github.com/kovidgoyal/kitty/issues/8680); [kitty launcher main.c](https://raw.githubusercontent.com/kovidgoyal/kitty/master/kitty/launcher/main.c)).
- footclient returns the child's exit code and reserves distinct codes (220, 230) for launcher and server failures ([footclient.1](https://codeberg.org/dnkl/foot/raw/branch/master/doc/footclient.1.scd)).
- WezTerm on Windows waits and calls `std::process::exit(status.code().unwrap_or(1))` ([wezterm src/main.rs](https://raw.githubusercontent.com/wezterm/wezterm/main/wezterm/src/main.rs)).
- Vim refuses to fork when it has jobs because they would no longer be its children, confirming that parentage is what makes waiting possible ([vim gui_x11.txt](https://raw.githubusercontent.com/vim/vim/master/runtime/doc/gui_x11.txt)).

### Inferences
- Race to close: SIGINT from Ctrl+C goes to the terminal's foreground process group, which includes the child until it calls `setsid()`. Block SIGINT, SIGQUIT, SIGTSTP, SIGHUP and SIGTERM with `sigprocmask` before `fork`; in the child call `setsid()` first, then restore default dispositions and unblock; in the parent install handlers, then unblock. This closes the window without a pipe for the wait case. For `-b`, still use kitty's pipe handshake (parent exits only after child has setsid), since the parent exits straight away.
- Launcher behaviour: on SIGINT (and SIGTERM, SIGHUP) stop waiting and `_exit(130)` (128 + SIGINT; for SIGHUP use 129). The window, in its own session, receives none of these. When the launcher exits, the window is reparented to init or the nearest subreaper (often systemd --user or the terminal), which reaps it; no zombie.
- Exit code mapping: `WIFEXITED` -> pass through `WEXITSTATUS`; `WIFSIGNALED` -> `128 + WTERMSIG` (shell convention), optionally printing "foo_edit: window crashed (SIGSEGV)". Reserve a launcher-only code (e.g. footclient-style) for "could not fork/spawn". Because waitpid returns when the process dies however it dies, the launcher cannot hang on a crash; it can only hang if the window process hangs, which is the user's call (Ctrl+C).
- pidfd (Linux 5.3+, `pidfd_open`, also returned by `clone3`/`CLONE_PIDFD`): lets the launcher `poll()` on the child alongside a `signalfd` or self-pipe, avoiding EINTR loops and PID-reuse races. Optional nicety; plain blocking `waitpid` with a SIGINT handler that `_exit`s is sufficient for a launcher whose only job is to wait. (pidfd details are prior knowledge, not sourced in this pass.)
- Do not use `PR_SET_PDEATHSIG` in the child: the requirement is the opposite (window survives launcher death).

### Gaps
- No primary source fetched for pidfd semantics or the exact POSIX rule that SIGHUP on terminal hangup goes to the session's foreground process group; these are standard but should be cited from `pidfd_open(2)` and `credentials(7)`/`termios` docs if the final report needs them.

## 5. Installing and presenting multiple binaries; packaging norms

### Takeaway
Shipping two executables is normal and accepted (WezTerm, Zed, foot, Alacritty's single binary with subcommands, kitty with a C launcher plus a Python payload). Users only ever type one name; the second binary is an implementation detail placed next to the first or in a libexec directory.

### Cited Findings
- WezTerm: user-facing `wezterm`, with `wezterm-gui` alongside it, located by the launcher as a sibling binary and exec'd ([wezterm src/main.rs](https://raw.githubusercontent.com/wezterm/wezterm/main/wezterm/src/main.rs)).
- Zed: a `cli` crate builds the user-facing `zed` command, separate from the app binary; on macOS the app is launched via Launch Services from the bundle ([zed cli main.rs](https://raw.githubusercontent.com/zed-industries/zed/main/crates/cli/src/main.rs)).
- foot: `foot` and `footclient` are both user-facing, with systemd units for the server ([foot.1](https://codeberg.org/dnkl/foot/raw/branch/master/doc/foot.1.scd); [footclient.1](https://codeberg.org/dnkl/foot/raw/branch/master/doc/footclient.1.scd)).
- kitty: a native C launcher is the `kitty` executable, which then runs the Python/C program in-process (from source read) ([kitty launcher main.c](https://raw.githubusercontent.com/kovidgoyal/kitty/master/kitty/launcher/main.c)).
- Neovide maintainers pushed back on putting launcher scripts in Homebrew because they sit outside Neovide's release cycle ([neovide PR #2512](https://github.com/neovide/neovide/pull/2512)).

### Inferences
- FHS convention for a helper that users should not run directly is `/usr/libexec/foo_edit/foo_edit-window` (or `/usr/lib/foo_edit/` on distros without libexec); the launcher finds it relative to `/proc/self/exe` with a compile-time fallback. This is prior knowledge; no distro policy citation fetched.
- On macOS the `.app` bundle's main executable must be the GUI; the CLI launcher can be a second executable inside `Contents/MacOS` or `Contents/Resources/bin` and symlinked into PATH (the Zed and VS Code pattern), and kitty/Neovide show exec is required there anyway.

### Gaps
- No Debian/Fedora policy text was fetched on single versus multiple binaries; I found no evidence that distros prefer single binaries.

## Recommendations (synthesised from the above)

1. Linux: the proposed single binary with fork-without-exec is defensible and has direct prior art (kitty). Conditions: fork as the first meaningful thing in `main` after argument parsing; assert `Threads: 1` from `/proc/self/status` and fall back to fork+exec (or `posix_spawn` with `POSIX_SPAWN_SETSID`) if not; no GLib/GIO/GTK, async runtime or logging threads before fork; child redirects stdio away from the terminal; child calls `setsid()` before unblocking signals.
2. Be honest about the latency trade: fork-without-exec gives one library load, same as the two-binary design, but the launcher itself is no longer 2.8 ms to main. If fast `--help`/error paths or a tiny `$EDITOR` shim matter, the two-binary WezTerm layout (libc-only launcher using `posix_spawn` + `POSIX_SPAWN_SETSID` or fork+setsid+exec, GUI in libexec) is equally fast to window and removes all fork-safety reasoning. Both are sound; the two-binary option is the lower-risk one.
3. macOS: fork+exec (or `posix_spawn` with `POSIX_SPAWN_SETSID` where available) with an env marker, per kitty; never run CF/AppKit in a forked child.
4. Signals: block SIGINT/SIGQUIT/SIGTSTP/SIGHUP/SIGTERM before fork; child setsid then restore; parent installs handlers that `_exit(128+sig)`; for `-b`, use kitty's pipe handshake before the parent exits (#8680).
5. Exit codes: pass through `WEXITSTATUS`, map signals to `128+n` with a one-line crash message, reserve a distinct code for launcher failure (footclient precedent). No IPC "exit" message is needed, which avoids the VS Code class of wrong-window and early-return bugs.
6. Default to waiting and keep detach opt-in (`-b`), matching Neovide 0.13+ and footclient; do not make detach depend on `isatty` silently (Neovide's redirection surprise).
