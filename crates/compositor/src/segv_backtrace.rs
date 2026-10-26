//! Opt-in crash probe for a real TTY device. glibc's unwinder is diagnostic only.
#![allow(unsafe_code, reason = "SIGSEGV alternate-stack diagnostics require libc FFI")]

use core::ptr;
use std::io;

const STACK_SIZE: usize = 64 * 1024;
const FRAMES: usize = 64;

extern "C" fn on_segv(signal: libc::c_int) {
    let message = b"TEHUTI_BACKTRACE_ON_SEGV: SIGSEGV; native frames follow\n";
    // SAFETY: `message` is static and stderr is available until process exit.
    let _written: isize = unsafe { libc::write(libc::STDERR_FILENO, message.as_ptr().cast(), message.len()) };
    let mut frames = [ptr::null_mut(); FRAMES];
    let capacity = libc::c_int::try_from(FRAMES).unwrap_or(64);
    // SAFETY: `frames` is writable for `capacity` addresses on the alternate stack.
    let count = unsafe { libc::backtrace(frames.as_mut_ptr(), capacity) };
    // SAFETY: The unwinder receives the addresses it just filled; diagnostic only.
    unsafe { libc::backtrace_symbols_fd(frames.as_ptr(), count, libc::STDERR_FILENO) };
    // SAFETY: A signal handler must not return after a segmentation fault.
    unsafe { libc::_exit(signal.saturating_add(128)) }
}

#[expect(clippy::redundant_pub_crate, reason = "parent module needs the installer")]
#[expect(
    clippy::as_conversions,
    reason = "libc sigaction stores the handler address as usize"
)]
pub(super) fn install_if_requested() -> io::Result<()> {
    if std::env::var_os("TEHUTI_BACKTRACE_ON_SEGV").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return Ok(());
    }

    // Force lazy unwinder setup outside the signal handler.
    let mut frame = ptr::null_mut();
    // SAFETY: `frame` is valid writable storage for one address.
    let _frames = unsafe { libc::backtrace(&raw mut frame, 1) };

    let stack = vec![0_u8; STACK_SIZE].into_boxed_slice();
    let stack = Box::leak(stack);
    let alternate = libc::stack_t {
        ss_sp: stack.as_mut_ptr().cast(),
        ss_flags: 0,
        ss_size: stack.len(),
    };
    // SAFETY: The alternate stack is leaked for the remaining process lifetime.
    if unsafe { libc::sigaltstack(&raw const alternate, ptr::null_mut()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: Zero is a valid initial sigaction before its mask is initialized.
    let mut action: libc::sigaction = unsafe { core::mem::zeroed() };
    action.sa_sigaction = (on_segv as *const ()).addr();
    action.sa_flags = libc::SA_ONSTACK | libc::SA_RESETHAND;
    // SAFETY: `action.sa_mask` is writable storage for a signal set.
    if unsafe { libc::sigemptyset(&raw mut action.sa_mask) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: The C handler and initialized action remain valid after install.
    if unsafe { libc::sigaction(libc::SIGSEGV, &raw const action, ptr::null_mut()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    tracing::warn!(
        "SIGSEGV backtrace enabled; use a debug build with symbols (RUSTFLAGS='-C force-frame-pointers=yes' cargo build -p tehuti)"
    );
    Ok(())
}
