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

    shell();
}

fn shell() -> ! {
    let _ = sys::api::print("sigma-init: type 'help'\n");
    let mut line = [0u8; 96];
    loop {
        let _ = sys::api::print("sigma$ ");
        // read a line via fd0 (PS/2 keyboard -> kernel)
        let mut n = 0usize;
        loop {
            let mut b = [0u8; 1];
            match sys::api::read(0, &mut b) {
                Ok(1) => {
                    let c = b[0];
                    if c == b'\n' {
                        let _ = sys::api::write(1, b"\n");
                        break;
                    } else if c == 0x7f {
                        if n > 0 {
                            n -= 1;
                            let _ = sys::api::write(1, b"\x08 \x08");
                        }
                    } else if n < line.len() {
                        line[n] = c;
                        n += 1;
                        let _ = sys::api::write(1, &b);
                    }
                }
                _ => core::hint::spin_loop(),
            }
        }
        let cmd = core::str::from_utf8(&line[..n]).unwrap_or("").trim();
        run(cmd);
    }
}

fn run(cmd: &str) {
    let mut it = cmd.split_whitespace();
    match it.next() {
        Some("help") | None => {
            let _ = sys::api::print("commands: ls <dir> | cat <file> | mem | exit\n");
        }
        Some("ls") => {
            let path = it.next().unwrap_or("/");
            let mut buf = [0u8; 512];
            match sys::api::listdir(path, &mut buf) {
                Ok(n) => {
                    let _ = sys::api::write(1, &buf[..n]);
                    let _ = sys::api::print("\n");
                }
                Err(e) => {
                    let _ = sys::api::print("ls: err ");
                    let _ = sys::api::print(&e.to_string());
                    let _ = sys::api::print("\n");
                }
            }
        }
        Some("cat") => {
            let Some(path) = it.next() else {
                let _ = sys::api::print("usage: cat <file>\n");
                return;
            };
            match sys::api::open(path, 0) {
                Ok(fd) => {
                    let mut buf = [0u8; 256];
                    loop {
                        match sys::api::read(fd, &mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                let _ = sys::api::write(1, &buf[..n]);
                            }
                            Err(_) => break,
                        }
                    }
                    let _ = sys::api::close(fd);
                }
                Err(e) => {
                    let _ = sys::api::print("cat: err ");
                    let _ = sys::api::print(&e.to_string());
                    let _ = sys::api::print("\n");
                }
            }
        }
        Some("mem") => {
            let _ = sys::api::print("sigma-init: kernel owns the memory map\n");
        }
        Some("exit") => {
            let _ = sys::api::print("sigma-init: bye\n");
            sys::api::exit(0);
        }
        Some(other) => {
            let _ = sys::api::print("unknown: ");
            let _ = sys::api::print(other);
            let _ = sys::api::print(" (try 'help')\n");
        }
    }
}
