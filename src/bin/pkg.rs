//! Simple package manager stub for SigmaOS.
//!
//! Placeholder for future alpm-rs integration.

#![no_std]
#![no_main]

extern crate alloc;
extern crate sigma_sys;

use sigma_sys::print;

#[no_mangle]
pub extern "C" fn main() -> i32 {
    let _ = print("SigmaOS Package Manager (pkg)\n");
    let _ = print("Status: Stub implementation\n");
    let _ = print("Future: alpm-rs integration for Arch-compatible package management\n");
    0
}
