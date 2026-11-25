//! sigma-init — SigmaOS-native PID 1 `/init`.
//!
//! A `no_std` static ELF built for `x86_64-unknown-none` that speaks
//! the kernel syscall ABI through `sigma-sys`. This is the real
//! initramfs `/init` — unlike the host `sigma-sh` binary it does
//! not need Linux syscalls, TLS, or a dynamic loader.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]


use sigma_sys as sys;

/// Print a u64 in decimal — stack buffer only, no allocator.
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

/// Print a u64 as 16 hex digits — stack buffer only, no allocator.
fn phex(v: u64) {
    let mut b = [0u8; 16];
    for i in 0..16 {
        let d = ((v >> ((15 - i) * 4)) & 0xf) as u8;
        b[i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
    }
    let _ = sys::api::write(1, &b);
}

/// Print a syscall error code.
fn perr(e: i64) {
    let _ = sys::api::print("err ");
    pdec(e as u64);
    let _ = sys::api::print("\n");
}

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
    pdec(pid as u64);
    let _ = sys::api::print("\nsigma-init: idle (halt)\n");

    // boot self-test: SPAWN -> WAIT process lifecycle
    match sys::api::spawn(b"/child") {
        Ok(pid) => {
            let _ = sys::api::print("spawned pid=");
            pdec(pid);
            let _ = sys::api::print("\n");
            // Child is preemptively scheduled — WAIT may return
            // EAGAIN until it exits; retry with a userspace spin
            // (each retry is a syscall boundary the scheduler uses).
            loop {
                match sys::api::wait(pid) {
                    Ok(code) => {
                        let _ = sys::api::print("wait status=");
                        pdec(code as u64);
                        let _ = sys::api::print("\n");
                        break;
                    }
                    Err(-11) => core::hint::spin_loop(), // EAGAIN: still running
                    Err(e) => {
                        perr(e);
                        break;
                    }
                }
            }
        }
        Err(e) => perr(e),
    }
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
            let _ = sys::api::print("commands: ls <dir> | cat <file> | ver | time | mem | spawn [path] | wait <pid> | exit\n");
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
                    perr(e);
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
                    perr(e);
                }
            }
        }
        Some("mem") => {
            // Exercise BRK: query, grow one page, touch it, report.
            match sys::api::brk(0) {
                Ok(cur) => {
                    let _ = sys::api::print("brk: cur=0x");
                    phex(cur);
                    let new = cur + 0x1000;
                    match sys::api::brk(new) {
                        Ok(nb) => {
                            let _ = sys::api::print(" grew=0x");
                            phex(nb);
                            // SAFETY: nb-0x1000..nb is freshly mapped
                            // anonymous memory owned by this process.
                            unsafe {
                                *((nb - 1) as *mut u8) = 0x5a;
                            }
                            let v = unsafe { *((nb - 1) as *mut u8) };
                            let _ = sys::api::print(" last=");
                            pdec(v as u64);
                            let _ = sys::api::print("\n");
                        }
                        Err(e) => {
                            let _ = sys::api::print("brk grow err ");
                            perr(e);
                        }
                    }
                }
                Err(e) => {
                    let _ = sys::api::print("brk err ");
                    perr(e);
                }
            }
            match sys::api::mmap(0, 0x2000) {
                Ok(a) => {
                    let _ = sys::api::print("mmap: base=0x");
                    phex(a);
                    match sys::api::munmap(a, 0x2000) {
                        Ok(r) => {
                            let _ = sys::api::print(" munmap=0x");
                            phex(r);
                        }
                        Err(e) => perr(e),
                    }
                    let _ = sys::api::print("\n");
                }
                Err(e) => {
                    let _ = sys::api::print("mmap err ");
                    perr(e);
                }
            }
        }
        Some("spawn") => {
            let path = it.next().unwrap_or("/child");
            match sys::api::spawn(path.as_bytes()) {
                Ok(pid) => {
                    let _ = sys::api::print("spawned pid=");
                    pdec(pid);
                    let _ = sys::api::print("\n");
                }
                Err(e) => perr(e),
            }
        }
        Some("wait") => {
            let Some(ps) = it.next() else {
                let _ = sys::api::print("usage: wait <pid>\n");
                return;
            };
            let pid = ps.bytes().fold(0u64, |a, b| {
                if b.is_ascii_digit() { a * 10 + (b - b'0') as u64 } else { a }
            });
            match sys::api::wait(pid) {
                Ok(code) => {
                    let _ = sys::api::print("wait: status=");
                    pdec(code as u64);
                    let _ = sys::api::print("\n");
                }
                Err(e) => perr(e),
            }
        }
        Some("ver") => {
            match sys::api::kernel_version() {
                Ok(v) => {
                    let _ = sys::api::print("kernel ");
                    let _ = sys::api::print(&v);
                    let _ = sys::api::print("\n");
                }
                Err(e) => {
                    let _ = sys::api::print("ver err ");
                    perr(e);
                }
            }
        }
        Some("time") => {
            match sys::api::time() {
                Ok(t) => {
                    let _ = sys::api::print("ticks=");
                    pdec(t);
                    let _ = sys::api::print("\n");
                }
                Err(e) => {
                    let _ = sys::api::print("time err ");
                    perr(e);
                }
            }
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
