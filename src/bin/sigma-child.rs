//! sigma-child — minimal SigmaOS-native user binary for SPAWN tests.
//! Prints its pid and exits with status 42.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use sigma_sys as sys;

struct Bump;
static mut HEAP: [u8; 16 * 1024] = [0; 16 * 1024];
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
    let _ = sys::api::print("sigma-child: panic\n");
    sys::api::exit(101);
}

fn pdec(mut v: u64) {
    let mut b = [0u8; 20];
    let mut i = b.len();
    if v == 0 {
        i -= 1;
        b[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        b[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    let _ = sys::api::write(1, &b[i..]);
}

#[unsafe(naked)]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov rdi, [rsp]",
        "lea rsi, [rsp + 8]",
        "and rsp, -16",
        "call {main}",
        "ud2",
        main = sym main_inner,
    );
}

fn main_inner(argc: u64, argv: *const *const u8) -> ! {
    sys::arch::install();
    let _ = sys::api::print("sigma-child: alive, pid=");
    pdec(sys::api::getpid().unwrap_or(0) as u64);
    let _ = sys::api::print(", argc=");
    pdec(argc);
    if !argv.is_null() {
        for i in 0..argc as usize {
            let p = unsafe { *argv.add(i) };
            if !p.is_null() {
                let mut s = alloc::string::String::new();
                unsafe {
                    let mut q = p;
                    while *q != 0 && s.len() < 64 {
                        s.push(*q as char);
                        q = q.add(1);
                    }
                }
                let _ = sys::api::print(&alloc::format!(" arg{}={}", i, s));
            }
        }
    }
    let _ = sys::api::print("\n");
    // Spin for a while so the timer IRQ preempts us and the parent
    // gets scheduled — demonstrates preemptive context switching.
    let mut i = 0u64;
    while i < 20_000_000 {
        i += 1;
        if i % 5_000_000 == 0 {
            let _ = sys::api::print("sigma-child: tick ");
            pdec(i);
            let _ = sys::api::print("\n");
        }
    }
    let _ = sys::api::print("sigma-child: exiting 42\n");
    sys::api::exit(42);
}
