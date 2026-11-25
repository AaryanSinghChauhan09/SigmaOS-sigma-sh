//! Package manager for SigmaOS.
//!
//! Basic package management commands (install, remove, search, list).
//! Full integration with pkg-core requires std library support.

#![no_std]
#![no_main]

extern crate alloc;
extern crate sigma_sys;

use alloc::string::String;
use sigma_sys::{print, listdir, open, read, close};

struct PkgCommand {
    name: String,
    args: alloc::vec::Vec<String>,
}

fn parse_args(args: &[&str]) -> PkgCommand {
    if args.is_empty() {
        return PkgCommand {
            name: String::from("help"),
            args: alloc::vec::Vec::new(),
        };
    }
    let name = String::from(args[0]);
    let cmd_args: alloc::vec::Vec<String> = args[1..].iter().map(|s| String::from(*s)).collect();
    PkgCommand { name, args: cmd_args }
}

fn cmd_help() {
    let _ = print("SigmaOS Package Manager (pkg)\n");
    let _ = print("Usage: pkg <command> [args]\n");
    let _ = print("\n");
    let _ = print("Commands:\n");
    let _ = print("  install <package>   Install a package\n");
    let _ = print("  remove <package>    Remove a package\n");
    let _ = print("  search <query>      Search for packages\n");
    let _ = print("  list               List installed packages\n");
    let _ = print("  update              Update package database\n");
    let _ = print("  upgrade             Upgrade all packages\n");
    let _ = print("  help                Show this help\n");
}

fn cmd_install(_package: &str) {
    let _ = print("pkg: install - stub implementation\n");
    let _ = print("Note: Full package management requires pkg-core integration\n");
}

fn cmd_remove(_package: &str) {
    let _ = print("pkg: remove - stub implementation\n");
    let _ = print("Note: Full package management requires pkg-core integration\n");
}

fn cmd_search(_query: &str) {
    let _ = print("pkg: search - stub implementation\n");
    let _ = print("Note: Full package management requires pkg-core integration\n");
}

fn cmd_list() {
    let _ = print("pkg: list - stub implementation\n");
    let _ = print("Note: Full package management requires pkg-core integration\n");
}

fn cmd_update() {
    let _ = print("pkg: update - stub implementation\n");
    let _ = print("Note: Full package management requires pkg-core integration\n");
}

fn cmd_upgrade() {
    let _ = print("pkg: upgrade - stub implementation\n");
    let _ = print("Note: Full package management requires pkg-core integration\n");
}

fn execute(cmd: &PkgCommand) {
    match cmd.name.as_str() {
        "help" => cmd_help(),
        "install" => {
            if cmd.args.is_empty() {
                let _ = print("pkg: install requires a package name\n");
            } else {
                cmd_install(&cmd.args[0]);
            }
        }
        "remove" => {
            if cmd.args.is_empty() {
                let _ = print("pkg: remove requires a package name\n");
            } else {
                cmd_remove(&cmd.args[0]);
            }
        }
        "search" => {
            if cmd.args.is_empty() {
                let _ = print("pkg: search requires a query\n");
            } else {
                cmd_search(&cmd.args[0]);
            }
        }
        "list" => cmd_list(),
        "update" => cmd_update(),
        "upgrade" => cmd_upgrade(),
        _ => {
            let _ = print("pkg: unknown command: ");
            let _ = print(&cmd.name);
            let _ = print("\n");
            cmd_help();
        }
    }
}

#[no_mangle]
pub extern "C" fn main() -> i32 {
    // Parse command line from kernel (args would be passed via spawn)
    // For now, just show help
    let cmd = PkgCommand {
        name: String::from("help"),
        args: alloc::vec::Vec::new(),
    };
    execute(&cmd);
    0
}
