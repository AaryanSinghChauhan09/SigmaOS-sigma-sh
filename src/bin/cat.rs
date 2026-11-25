//! Simple cat utility for SigmaOS.
//!
//! Reads and prints file contents.

#![no_std]
#![no_main]

extern crate alloc;
extern crate sigma_sys;

use sigma_sys::{open, read, close, print};

#[no_mangle]
pub extern "C" fn main() -> i32 {
    let args = ["cat", "/"];
    let path = if args.len() > 1 { args[1] } else { "/" };
    
    match open(path, 0) {
        Ok(fd) => {
            let mut buf = [0u8; 4096];
            loop {
                match read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = print(alloc::string::String::from_utf8_lossy(&buf[..n]).as_ref());
                    }
                    Err(_) => {
                        let _ = print("cat: read error\n");
                        let _ = close(fd);
                        return 1;
                    }
                }
            }
            let _ = close(fd);
            0
        }
        Err(_) => {
            let _ = print("cat: failed to open file\n");
            1
        }
    }
}
