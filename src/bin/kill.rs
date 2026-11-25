//! Process termination utility for SigmaOS.
//!
//! Send signals to processes (SIGTERM, SIGKILL).

#![no_std]
#![no_main]

extern crate alloc;
extern crate sigma_sys;

use alloc::string::String;
use sigma_sys::{print, getpid};

/// Signal constants.
const SIGTERM: u64 = 15;
const SIGKILL: u64 = 9;

fn cmd_help() {
    let _ = print("SigmaOS kill - send signals to processes\n");
    let _ = print("Usage: kill [-signal] <pid>\n");
    let _ = print("\n");
    let _ = print("Signals:\n");
    let _ = print("  -TERM, -15   Terminate process (default)\n");
    let _ = print("  -KILL, -9    Force kill process\n");
    let _ = print("  -HUP, -1     Hangup\n");
    let _ = print("  -INT, -2     Interrupt\n");
}

fn parse_signal(arg: &str) -> Option<u64> {
    match arg {
        "-TERM" | "-15" => Some(SIGTERM),
        "-KILL" | "-9" => Some(SIGKILL),
        "-HUP" | "-1" => Some(1),
        "-INT" | "-2" => Some(2),
        _ => None,
    }
}

fn parse_pid(arg: &str) -> Option<u64> {
    arg.parse::<u64>().ok()
}

fn send_signal(pid: u64, signal: u64) {
    // Note: Full signal implementation requires kernel signal support
    // For now, this is a stub that prints what would happen
    let _ = print("kill: sending signal ");
    let _ = print(&format_u64(signal));
    let _ = print(" to PID ");
    let _ = print(&format_u64(pid));
    let _ = print("\n");
    let _ = print("Note: Full signal support requires kernel signal implementation\n");
}

fn format_u64(v: u64) -> String {
    let mut buf = alloc::string::String::new();
    if v == 0 {
        buf.push('0');
    } else {
        let mut n = v;
        let mut digits = alloc::vec::Vec::new();
        while n > 0 {
            digits.push((n % 10) as u8 + b'0');
            n /= 10;
        }
        while let Some(d) = digits.pop() {
            buf.push(d as char);
        }
    }
    buf
}

#[no_mangle]
pub extern "C" fn main() -> i32 {
    // For now, just show help (args would be passed via spawn)
    cmd_help();
    0
}
