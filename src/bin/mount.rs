//! Mount utility for SigmaOS.
//!
//! Mount filesystems (requires kernel mount syscall support).

#![no_std]
#![no_main]
#![allow(dead_code)]

extern crate alloc;
extern crate sigma_sys;

use alloc::string::String;
use sigma_sys::print;

fn cmd_help() {
    let _ = print("SigmaOS mount - mount filesystems\n");
    let _ = print("Usage: mount [-t <type>] <device> <mountpoint>\n");
    let _ = print("\n");
    let _ = print("Options:\n");
    let _ = print("  -t <type>   Filesystem type (ext4, fat32, etc.)\n");
    let _ = print("  -o <opts>   Mount options (ro, rw, etc.)\n");
    let _ = print("\n");
    let _ = print("Note: Full mount support requires kernel mount syscall\n");
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

fn cmd_mount(device: &str, mountpoint: &str, fstype: Option<&str>, _opts: Option<&str>) {
    let _ = print("mount: mounting ");
    let _ = print(device);
    let _ = print(" at ");
    let _ = print(mountpoint);
    let _ = print("\n");
    
    if let Some(fs) = fstype {
        let _ = print("  type: ");
        let _ = print(fs);
        let _ = print("\n");
    }
    
    let _ = print("Note: Full mount support requires kernel mount syscall\n");
}

fn cmd_umount(mountpoint: &str) {
    let _ = print("mount: unmounting ");
    let _ = print(mountpoint);
    let _ = print("\n");
    let _ = print("Note: Full umount support requires kernel umount syscall\n");
}

#[no_mangle]
pub extern "C" fn main() -> i32 {
    // For now, just show help (args would be passed via spawn)
    cmd_help();
    0
}
