//! sigma-sh binary — rescue shell frontend.
//!
//! Uses the host std implementations of the Io/Fs/ProcTable/MemInfo/
//! System traits. On SigmaOS the same traits are implemented over
//! sigma-sys syscalls.

use sigma_sh::{Fs, Io, MemInfo, ProcTable, Shell, System};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::time::{SystemTime, UNIX_EPOCH};

struct StdIo;
impl Io for StdIo {
    fn read_line(&mut self) -> Option<String> {
        let mut s = String::new();
        match io::stdin().lock().read_line(&mut s) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(s.trim_end_matches('\n').to_string()),
        }
    }
    fn write(&mut self, s: &str) {
        let _ = io::stdout().write_all(s.as_bytes());
        let _ = io::stdout().flush();
    }
}

struct StdFs {
    cwd: PathBuf,
}
impl Fs for StdFs {
    fn list(&self, path: &str) -> Option<Vec<String>> {
        let p = if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };
        let rd = std::fs::read_dir(p).ok()?;
        let mut v: Vec<String> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        Some(v)
    }
    fn read_to_string(&self, path: &str) -> Option<String> {
        let p = if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };
        std::fs::read_to_string(p).ok()
    }
    fn write_file(&self, path: &str, data: &str, append: bool) -> bool {
        use std::fs::OpenOptions;
        let p = if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };
        let r = if append {
            OpenOptions::new().create(true).append(true).open(&p)
        } else {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&p)
        };
        match r {
            Ok(mut f) => f.write_all(data.as_bytes()).is_ok(),
            Err(_) => false,
        }
    }
    fn create_dir(&self, path: &str) -> bool {
        let p = if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };
        std::fs::create_dir(p).is_ok()
    }
    fn remove_path(&self, path: &str) -> bool {
        let p = if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };
        std::fs::remove_file(&p).is_ok() || std::fs::remove_dir(&p).is_ok()
    }
    fn cwd(&self) -> String {
        self.cwd.display().to_string()
    }
}

struct StdProcs;
impl ProcTable for StdProcs {
    fn list(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Ok(out) = Proc::new("ps").arg("-eo").arg("pid,comm").output() {
            for line in String::from_utf8_lossy(&out.stdout).lines().skip(1) {
                v.push(line.trim().to_string());
            }
        }
        v
    }
}

struct StdMem;
impl MemInfo for StdMem {
    fn memory_kb(&self) -> (u64, u64) {
        // /proc/meminfo on Linux
        if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
            let mut total = 0u64;
            let mut avail = 0u64;
            for line in s.lines() {
                if line.starts_with("MemTotal:") {
                    total = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                } else if line.starts_with("MemAvailable:") {
                    avail = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                }
            }
            if total > 0 {
                return (total, avail);
            }
        }
        (0, 0)
    }
}

struct StdSys;
impl System for StdSys {
    fn reboot(&self) {
        let _ = Proc::new("systemctl").arg("reboot").status();
        let _ = Proc::new("reboot").status();
    }
    fn launch(&self, path: &str, args: &[String]) -> Result<i64, String> {
        let p = if Path::new(path).is_absolute() {
            path.to_string()
        } else {
            format!("/usr/bin/{}", path)
        };
        let child = Proc::new(&p)
            .args(args)
            .spawn()
            .map_err(|e| format!("{}: {}", p, e))?;
        Ok(child.id() as i64)
    }
    fn uptime(&self) -> u64 {
        let up = std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next().map(str::to_string));
        if let Some(u) = up {
            return u.parse::<f64>().unwrap_or(0.0) as u64;
        }
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
    fn sleep(&self, secs: u64) {
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }
}

fn main() {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let mut shell = Shell::new(StdIo, StdFs { cwd }, StdProcs, StdMem, StdSys);
    shell.run();
}
