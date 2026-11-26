//! Simple ls utility for SigmaOS.
//!
//! Lists directory contents using the listdir syscall.

#![no_std]
#![no_main]

extern crate alloc;
extern crate sigma_sys;

use alloc::string::String;
use sigma_sys::{listdir, print};

#[no_mangle]
pub extern "C" fn main() -> i32 {
    let args = ["ls", "/"];
    let path = if args.len() > 1 { args[1] } else { "/" };

    let mut buf = [0u8; 4096];
    match listdir(path, &mut buf) {
        Ok(n) => {
            let output = String::from_utf8_lossy(&buf[..n]);
            let _ = print(&output);
            0
        }
        Err(_) => {
            let _ = print("ls: failed to list directory\n");
            1
        }
    }
}
