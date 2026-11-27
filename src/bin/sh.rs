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

    fn write_file(&self, path: &str, data: &str, append: bool) -> bool {
        // Linux open(2) bits honored by the kernel: O_WRONLY=1,
        // O_CREAT=0x40, O_TRUNC=0x200, O_APPEND=0x400.
        let flags = 1 | 0x40 | if append { 0x400 } else { 0x200 };
        let Ok(fd) = sys::api::open(path, flags) else {
            return false;
        };
        let ok = sys::api::write(fd, data.as_bytes()).is_ok();
        let _ = sys::api::close(fd);
        ok
    }

    fn create_dir(&self, path: &str) -> bool {
        sys::api::mkdir(path).is_ok()
    }

    fn remove_path(&self, path: &str) -> bool {
        // rm uses unlink for files; rmdir builtin calls this too —
        // try unlink first, then rmdir for directories.
        sys::api::unlink(path)
            .or_else(|_| sys::api::remove_dir(path))
            .is_ok()
    }

    fn stat(&self, path: &str) -> Option<(bool, u64)> {
        sys::api::stat(path).ok().map(|s| (s.is_dir(), s.size))
    }

    fn chdir(&self, path: &str) -> bool {
        sys::api::chdir(path).is_ok()
    }

    fn open_write(&self, path: &str, append: bool) -> Option<i64> {
        const O_WRONLY: u32 = 1;
        const O_CREAT: u32 = 0x40;
        const O_TRUNC: u32 = 0x200;
        const O_APPEND: u32 = 0x400;
        let flags = O_WRONLY | O_CREAT | if append { O_APPEND } else { O_TRUNC };
        sys::api::open(path, flags).ok()
    }

    fn open_read(&self, path: &str) -> Option<i64> {
        sys::api::open(path, 0).ok()
    }

    fn cwd(&self) -> String {
        let mut buf = [0u8; 128];
        match sys::api::getcwd(&mut buf) {
            Ok(n) => String::from_utf8_lossy(&buf[..n]).into_owned(),
            Err(_) => "/".to_string(),
        }
    }

    fn pipe(&self) -> Option<(i64, i64)> {
        sys::api::pipe().ok()
    }

    fn close(&self, fd: i64) -> bool {
        sys::api::close(fd).is_ok()
    }

    fn read_fd(&self, fd: i64, buf: &mut [u8]) -> i64 {
        match sys::api::read(fd, buf) {
            Ok(n) => n as i64,
            Err(e) => e,
        }
    }

    fn write_fd(&self, fd: i64, buf: &[u8]) -> i64 {
        match sys::api::write(fd, buf) {
            Ok(n) => n as i64,
            Err(e) => e,
        }
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

    fn launch(
        &self,
        path: &str,
        args: &[String],
        env: &[String],
        stdout_fd: i64,
        stdin_fd: i64,
        stderr_fd: i64,
    ) -> Result<i64, String> {
        let arg_refs: Vec<&[u8]> = args.iter().map(|a| a.as_bytes()).collect();
        let env_refs: Vec<&[u8]> = env.iter().map(|e| e.as_bytes()).collect();
        sys::api::spawn_args(
            path.as_bytes(),
            &arg_refs,
            &env_refs,
            stdout_fd.max(0) as u64,
            stdin_fd.max(0) as u64,
            stderr_fd.max(0) as u64,
        )
        .map(|pid| pid as i64)
        .map_err(|e| format!("spawn failed ({})", e))
    }

    fn wait(&self, pid: i64) -> Result<i64, String> {
        sys::api::wait(pid as u64).map_err(|e| format!("wait failed ({})", e))
    }

    fn wait_any(&self) -> Result<i64, String> {
        sys::api::wait_any().map_err(|e| format!("wait failed ({})", e))
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

    fn fork(&self) -> i64 {
        sigma_sys::api::fork().unwrap_or(-1)
    }

    fn exec(&self, path: &str, args: &[String], env: &[String]) -> Result<(), String> {
        let arg_refs: Vec<&str> = args.iter().map(|a| a.as_str()).collect();
        let env_refs: Vec<&str> = env.iter().map(|e| e.as_str()).collect();
        sigma_sys::api::exec(path, &arg_refs, &env_refs).map_err(|e| format!("exec failed ({})", e))
    }

    fn kill(&self, pid: i64, sig: &str) -> bool {
        // Fatal-by-default set — must match the kernel's FATAL table.
        let code = match sig {
            "HUP" => 1,
            "INT" => 2,
            "QUIT" => 3,
            "ABRT" => 6,
            "KILL" => 9,
            "USR1" => 10,
            "USR2" => 12,
            "ALRM" => 14,
            "TERM" => 15,
            _ => match sig.parse::<i64>() {
                Ok(n) if (1..=15).contains(&n) => n,
                _ => return false,
            },
        };
        sys::api::kill(pid, code).is_ok()
    }

    fn uname(&self) -> &'static str {
        "sigmaos"
    }

    fn kernel_release(&self) -> String {
        // kernel_version() allocates; return an owned String.
        sys::api::kernel_version()
            .map(|v| v.to_string())
            .unwrap_or_else(|_| "unknown".into())
    }

    fn dmesg(&self) -> Option<String> {
        let mut buf = alloc::vec![0u8; sigma_sys::api::DMESG_CAP];
        match sys::api::dmesg(&mut buf) {
            Ok(n) => Some(alloc::string::String::from_utf8_lossy(&buf[..n]).into_owned()),
            Err(_) => None,
        }
    }

    fn statfs(&self, path: &str) -> Option<(u64, u64)> {
        sys::api::statfs(path).ok()
    }

    fn mount(&self, source: &str, target: &str, fstype: &str) -> Result<(), String> {
        sys::api::mount(source, target, fstype).map_err(|e| format!("mount: {}", e))
    }

    fn umount(&self, target: &str) -> Result<(), String> {
        sys::api::umount(target).map_err(|e| format!("umount: {}", e))
    }

    fn mountlist(&self) -> Option<Vec<(String, u64, u64)>> {
        sys::api::mountlist()
            .ok()
            .map(|ms| ms.into_iter().map(|m| (m.path, m.kind, m.dev)).collect())
    }
}

/// Naked entry: the kernel hands us a 16-byte-aligned RSP, but a
/// normal `extern "C"` fn expects post-call alignment (rsp % 16 == 8).
/// Align here, then `call` so the callee sees the ABI-correct value.
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

fn main_inner(_argc: u64, _argv: *const *const u8) -> ! {
    sys::arch::install();
    let mut shell = Shell::new(SysIo, SysFs, SysProcs, SysMem, SysSystem);
    shell.run();
    sys::api::exit(0);
}
