//! Simple ps utility for SigmaOS.
//!
//! Lists running processes.

#![no_std]
#![no_main]

extern crate alloc;
extern crate sigma_sys;

use sigma_sys::{getpid, print};

#[no_mangle]
pub extern "C" fn main() -> i32 {
    let _ = print("PID  STATE\n");
    match getpid() {
        Ok(pid) => {
            let line = alloc::format!("{}  running\n", pid);
            let _ = print(&line);
            0
        }
        Err(_) => {
            let _ = print("ps: failed to get pid\n");
            1
        }
    }
}
