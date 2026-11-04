//! SIGTERM, delivered on the main run loop. A `sigaction` handler writes
//! one byte to a self-pipe (the only thing it does: `write` is
//! async-signal-safe); a `CFFileDescriptor` source on the main run loop
//! watches the pipe's read end, and its callback runs the function
//! `install` was given, outside the handler, where the editor may run.

use std::ffi::c_void;
use std::io;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};

use objc2_core_foundation::{
    CFFileDescriptor, CFOptionFlags, CFRunLoop, kCFFileDescriptorReadCallBack,
    kCFRunLoopCommonModes,
};

/// The pipe's write end, for the handler; -1 until `install`.
static WRITE_FD: AtomicI32 = AtomicI32::new(-1);

/// What runs on the main thread once SIGTERM arrives.
static ON_TERM: OnceLock<fn()> = OnceLock::new();

/// Makes SIGTERM call `on_term` on the main run loop (in every mode, so a
/// live resize or a menu does not hold it back). Call it once, on the
/// main thread.
pub fn install(on_term: fn()) -> io::Result<()> {
    let _ = ON_TERM.set(on_term);
    let read = pipe()?;
    // SAFETY: `readable` has the callback's signature and takes no
    // context; the descriptor is ours and is closed with the source.
    let fd = unsafe { CFFileDescriptor::new(None, read, true, Some(readable), std::ptr::null()) }
        .ok_or_else(|| io::Error::other("no CFFileDescriptor for the signal pipe"))?;
    let source = CFFileDescriptor::new_run_loop_source(None, Some(&fd), 0)
        .ok_or_else(|| io::Error::other("no run loop source for the signal pipe"))?;
    let main = CFRunLoop::main().ok_or_else(|| io::Error::other("no main run loop"))?;
    // SAFETY: a constant CoreFoundation exports.
    main.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
    fd.enable_call_backs(kCFFileDescriptorReadCallBack);
    // Both live as long as the process.
    std::mem::forget(source);
    std::mem::forget(fd);
    handle_sigterm()
}

/// A pipe for the handler, both ends non-blocking and closed on exec.
/// Returns the read end.
fn pipe() -> io::Result<libc::c_int> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors `pipe` writes.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for fd in fds {
        // SAFETY: `fd` is a descriptor `pipe` just made.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    WRITE_FD.store(fds[1], Ordering::Release);
    Ok(fds[0])
}

/// Points SIGTERM at `on_sigterm`.
fn handle_sigterm() -> io::Result<()> {
    // SAFETY: a zeroed sigaction is valid; the mask is then set explicitly
    // and the handler is an async-signal-safe extern "C" fn.
    let rc = unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_sigterm as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut())
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The handler: one byte down the pipe, `errno` as it was. A full pipe
/// already has a wake waiting, so a failed write loses nothing.
extern "C" fn on_sigterm(_: libc::c_int) {
    // SAFETY: `__error` is this thread's errno, saved and put back so the
    // code the signal interrupted never sees ours; `write` is
    // async-signal-safe and is given one valid byte.
    unsafe {
        let errno = *libc::__error();
        let fd = WRITE_FD.load(Ordering::Acquire);
        if fd >= 0 {
            let byte = 1u8;
            libc::write(fd, (&raw const byte).cast::<c_void>(), 1);
        }
        *libc::__error() = errno;
    }
}

/// The pipe can be read: empties it, asks for the next callback (each is
/// one-shot), then runs the function `install` was given.
unsafe extern "C-unwind" fn readable(f: *mut CFFileDescriptor, _: CFOptionFlags, _: *mut c_void) {
    // SAFETY: CoreFoundation passes the descriptor the callback is on.
    let Some(f) = (unsafe { f.as_ref() }) else {
        return;
    };
    drain(f.native_descriptor());
    f.enable_call_backs(kCFFileDescriptorReadCallBack);
    if let Some(on_term) = ON_TERM.get() {
        on_term();
    }
}

/// Reads `fd` until it would block.
fn drain(fd: libc::c_int) {
    let mut buf = [0u8; 64];
    // SAFETY: `buf` is valid for `buf.len()` bytes.
    while unsafe { libc::read(fd, buf.as_mut_ptr().cast::<c_void>(), buf.len()) } > 0 {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real SIGTERM, raised in this process, reaches the pipe; the
    /// default disposition is put back after.
    #[test]
    fn sigterm_reaches_the_pipe() {
        let read = pipe().unwrap();
        handle_sigterm().unwrap();
        // SAFETY: SIGTERM is handled now; `raise` runs the handler on this
        // thread before returning.
        unsafe { libc::raise(libc::SIGTERM) };
        let mut byte = 0u8;
        // SAFETY: one valid byte to read into.
        let n = unsafe { libc::read(read, (&raw mut byte).cast::<c_void>(), 1) };
        // SAFETY: putting back the default disposition and closing ours.
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::close(read);
            libc::close(WRITE_FD.swap(-1, Ordering::AcqRel));
        }
        assert_eq!((n, byte), (1, 1));
    }
}
