//! sh — SigmaOS-native rescue shell for bare metal.
//!
//! Runs the full `sigma_sh::Shell` — every builtin, pipelines,
//! redirection, globbing, history — over sigma-sys syscalls on
//! `x86_64-unknown-none`. The filesystem is the read-only initramfs
//! VFS, so write/mkdir/rm builtins report unsupported; `ps`/`mem`
//! have no kernel backing yet and show empty output.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};

use sigma_sh::{Fs, Io, MemInfo, ProcTable, Shell, System};
use sigma_sys as sys;

struct Bump;
static mut HEAP: [u8; 512 * 1024] = [0; 512 * 1024];
static mut OFF: usize = 0;

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe {
            let align = layout.align().max(8);
            let start = (*core::ptr::addr_of_mut!(OFF) + align - 1) & !(align - 1);
            let end = start + layout.size();
            if end > HEAP.len() {
                return core::ptr::null_mut();
            }
            *core::ptr::addr_of_mut!(OFF) = end;
            HEAP.as_mut_ptr().add(start)
        }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOC: Bump = Bump;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    let _ = sys::api::print("sh: panic\n");
    sys::api::exit(101);
}

// ── Io over console fd 0/1 ────────────────────────────────────────────

struct SysIo;

impl Io for SysIo {
    fn read_line(&mut self) -> Option<String> {
        let mut line = Vec::new();
        loop {
            let mut b = [0u8; 1];
            match sys::api::read(0, &mut b) {
                Ok(1) => match b[0] {
                    b'\n' => {
                        let _ = sys::api::write(1, b"\n");
                        return Some(String::from_utf8_lossy(&line).into_owned());
                    }
                    0x7f => {
                        if line.pop().is_some() {
                            let _ = sys::api::write(1, b"\x08 \x08");
                        }
                    }
                    c if c < 0x20 || c == 0x7f => {}
                    _ => {
                        line.push(b[0]);
                        let _ = sys::api::write(1, &b);
                    }
                },
                Ok(_) => core::hint::spin_loop(),
                Err(-11) => core::hint::spin_loop(), // EAGAIN: no input yet
                Err(_) => {
                    return if line.is_empty() {
                        None
                    } else {
                        Some(String::from_utf8_lossy(&line).into_owned())
                    };
                }
            }
        }
    }

    fn write(&mut self, s: &str) {
        let _ = sys::api::write(1, s.as_bytes());
    }
}

// ── Fs over the read-only initramfs VFS ───────────────────────────────

struct SysFs;

impl Fs for SysFs {
    fn list(&self, path: &str) -> Option<Vec<String>> {
        let mut buf = alloc::vec![0u8; 8192];
        let n = sys::api::listdir(path, &mut buf).ok()?;
        let text = String::from_utf8_lossy(&buf[..n]);
        Some(
            text.lines()
                .filter(|l| !l.is_empty())
                .map(ToString::to_string)
                .collect(),
        )
    }

    fn read_to_string(&self, path: &str) -> Option<String> {
        let fd = sys::api::open(path, 0).ok()?;
        let mut out = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match sys::api::read(fd, &mut chunk) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
            if out.len() > 1 << 20 {
                break; // cap at 1 MiB
            }
        }
        let _ = sys::api::close(fd);
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    // write_file/create_dir/remove_path keep trait defaults (unsupported):
    // the initramfs VFS is read-only.

    fn cwd(&self) -> String {
        "/".to_string()
    }
}

// ── Process table / memory: PROCINFO + MEMINFO syscalls ───────────────

struct SysProcs;
impl ProcTable for SysProcs {
    fn list(&self) -> Vec<String> {
        match sys::api::procinfo() {
            Ok(slots) => slots
                .iter()
                .filter(|(pid, _, _)| *pid != 0)
                .map(|(pid, flags, code)| {
                    let state = if flags & 1 != 0 {
                        if flags & 4 != 0 {
                            "reaped".to_string()
                        } else {
                            format!("zombie({})", code)
                        }
                    } else if flags & 2 != 0 {
                        "sleeping".to_string()
                    } else {
                        "running".to_string()
                    };
                    format!("{:>4} {}", pid, state)
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

struct SysMem;
impl MemInfo for SysMem {
    fn memory_kb(&self) -> (u64, u64) {
        // MEMINFO returns 4 KiB frames; convert to KiB.
        match sys::api::meminfo() {
            Ok((total, free)) => (total * 4, free * 4),
            Err(_) => (0, 0),
        }
    }
}

// ── System services over syscalls ─────────────────────────────────────

/// Approximate TSC frequency for sleep/uptime math (QEMU ~3 GHz).
const TSC_HZ: u64 = 3_000_000_000;

fn tsc() -> u64 {
    sys::api::time().unwrap_or(0)
}

struct SysSystem;

impl System for SysSystem {
    fn reboot(&self) {
        let _ = sys::api::print("sh: reboot: no kernel reboot syscall\n");
    }

    fn launch(&self, path: &str, _args: &[String]) -> Result<i64, String> {
        // SPAWN takes a path only (no argv yet); args are ignored.
        sys::api::spawn(path.as_bytes())
            .map(|pid| pid as i64)
            .map_err(|e| format!("spawn failed ({})", e))
    }

    fn uptime(&self) -> u64 {
        tsc() / TSC_HZ
    }

    fn sleep(&self, secs: u64) {
        let target = tsc() + secs.saturating_mul(TSC_HZ);
        while tsc() < target {
            core::hint::spin_loop();
        }
    }

    fn kill(&self, pid: i64, sig: &str) -> bool {
        let code = sig.parse::<i64>().unwrap_or(match sig {
            "HUP" => 1,
            "INT" => 2,
            "QUIT" => 3,
            "KILL" => 9,
            "USR1" => 10,
            "SEGV" => 11,
            "USR2" => 12,
            "TERM" => 15,
            "STOP" => 19,
            "CONT" => 18,
            _ => 15,
        });
        sys::api::kill(pid, code).is_ok()
    }

    fn uname(&self) -> &'static str {
        // kernel_version() allocates; leak the small string once per call.
        match sys::api::kernel_version() {
            Ok(v) => Box::leak(v.into_boxed_str()),
            Err(_) => "sigmaos",
        }
    }
}

/// Naked entry: the kernel hands us a 16-byte-aligned RSP, but a
/// normal `extern "C"` fn expects post-call alignment (rsp % 16 == 8).
/// Align here, then `call` so the callee sees the ABI-correct value.
#[unsafe(naked)]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "and rsp, -16",
        "call {main}",
        "ud2",
        main = sym main_inner,
    );
}

fn main_inner() -> ! {
    sys::arch::install();
    let mut shell = Shell::new(SysIo, SysFs, SysProcs, SysMem, SysSystem);
    shell.run();
    sys::api::exit(0);
}
