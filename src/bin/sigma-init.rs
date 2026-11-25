//! sigma-init — SigmaOS-native PID 1 `/init`.
//!
//! A `no_std` static ELF built for `x86_64-unknown-none` that speaks
//! the kernel syscall ABI through `sigma-sys`. This is the real
//! initramfs `/init` — unlike the host `sigma-sh` binary it does
//! not need Linux syscalls, TLS, or a dynamic loader.

#![no_std]
#![no_main]

extern crate alloc;

use sigma_sys as sys;
use alloc::string::ToString;

use core::alloc::{GlobalAlloc, Layout};

/// Tiny bump allocator over a static arena — enough for init strings.
struct Bump;
static mut HEAP: [u8; 64 * 1024] = [0; 64 * 1024];
static mut OFF: usize = 0;

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let off = unsafe { OFF };
        let align = l.align().max(16);
        let start = (off + align - 1) & !(align - 1);
        let end = start + l.size();
        if end > HEAP.len() {
            return core::ptr::null_mut();
        }
        unsafe { OFF = end };
        unsafe { HEAP.as_mut_ptr().add(start) }
    }
    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}
}

#[global_allocator]
static ALLOC: Bump = Bump;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // Best-effort: try to report via syscall before halting.
    let _ = sys::api::print("sigma-init: panic\n");
    sys::api::exit(101);
}

/// Process entry — the kernel jumps here with an empty user stack.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    sys::arch::install();
    let _ = sys::api::print("sigma-init: PID 1 alive\n");
    let pid = sys::api::getpid().unwrap_or(-1);
    let _ = sys::api::print("sigma-init: pid=");
    let _ = sys::api::print(&pid.to_string());
    let _ = sys::api::print("\nsigma-init: idle (halt)\n");
    loop {
        core::hint::spin_loop();
    }
}
