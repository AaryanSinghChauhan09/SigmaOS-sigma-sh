//! SigmaOS sigma-sh — minimal rescue shell.
//!
//! Builtins: help, ls, pwd, echo, cat, which, mem, ps, time, launch, reboot,
//! exit. I/O and filesystem access go through traits so the shell is
//! fully testable on host and pluggable to sigma-sys at runtime.

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

#[cfg(test)]
extern crate std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Input/output abstraction.
pub trait Io {
    /// Read one line (without newline). None = EOF.
    fn read_line(&mut self) -> Option<String>;
    /// Write output (no newline added).
    fn write(&mut self, s: &str);
    /// Write output + newline.
    fn writeln(&mut self, s: &str) {
        self.write(s);
        self.write("\n");
    }
}

/// Filesystem view for ls/cat/pwd.
pub trait Fs {
    /// List names in a directory.
    fn list(&self, path: &str) -> Option<Vec<String>>;
    /// Read a file's contents as string (lossy).
    fn read_to_string(&self, path: &str) -> Option<String>;
    /// Current directory.
    fn cwd(&self) -> String;
}

/// Process table view for `ps`.
pub trait ProcTable {
    /// Rows of "pid name state".
    fn list(&self) -> Vec<String>;
}

/// Memory info for `mem`.
pub trait MemInfo {
    /// (total_kb, free_kb).
    fn memory_kb(&self) -> (u64, u64);
}

/// System control for reboot/launch.
pub trait System {
    /// Reboot the machine.
    fn reboot(&self);
    /// Spawn a program; returns pid or error string.
    fn launch(&self, path: &str, args: &[String]) -> Result<i64, String>;
    /// Current uptime seconds.
    fn uptime(&self) -> u64;
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub args: Vec<String>,
}

/// A pipeline of commands connected by pipes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>,
}

/// Tokenize a command line: whitespace-separated, double-quote aware.
pub fn parse(line: &str) -> Option<Command> {
    let mut chars = line.chars().peekable();
    let mut name = String::new();
    // skip leading ws
    while chars.peek().is_some_and(|c| c.is_whitespace()) {
        chars.next();
    }
    // read command name
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            break;
        }
        name.push(c);
        chars.next();
    }
    if name.is_empty() {
        return None;
    }
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in chars {
        match c {
            '"' => in_quote = !in_quote,
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    args.push(core::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        args.push(cur);
    }
    Some(Command { name, args })
}

/// Parse a pipeline (commands separated by |).
pub fn parse_pipeline(line: &str) -> Option<Pipeline> {
    let commands: Vec<Command> = line.split('|').filter_map(parse).collect();

    if commands.is_empty() {
        None
    } else {
        Some(Pipeline { commands })
    }
}

/// The shell.
pub struct Shell<I, F, P, M, S> {
    pub io: I,
    pub fs: F,
    pub procs: P,
    pub mem: M,
    pub sys: S,
    pub cwd: String,
    pub running: bool,
}

impl<I, F, P, M, S> Shell<I, F, P, M, S>
where
    I: Io,
    F: Fs,
    P: ProcTable,
    M: MemInfo,
    S: System,
{
    /// Create a shell rooted at `/`.
    pub fn new(io: I, fs: F, procs: P, mem: M, sys: S) -> Self {
        Self {
            io,
            fs,
            procs,
            mem,
            sys,
            cwd: "/".into(),
            running: true,
        }
    }

    /// Run the REPL until EOF or `exit`.
    pub fn run(&mut self) {
        self.io.writeln(&format!(
            "sigma-sh {} — type 'help'",
            env!("CARGO_PKG_VERSION")
        ));
        while self.running {
            self.io.write("sigma> ");
            let Some(line) = self.io.read_line() else {
                break;
            };
            self.execute(&line);
        }
    }

    /// Execute one command line.
    pub fn execute(&mut self, line: &str) {
        // Try to parse as pipeline first
        if let Some(pipeline) = parse_pipeline(line) {
            self.execute_pipeline(&pipeline);
            return;
        }

        // Fall back to single command
        let Some(cmd) = parse(line) else {
            return;
        };
        match cmd.name.as_str() {
            "help" | "?" => self.cmd_help(),
            "exit" | "logout" | "quit" => self.running = false,
            "echo" => self.io.writeln(&cmd.args.join(" ")),
            "pwd" => self.io.writeln(&self.cwd),
            "ls" => self.cmd_ls(&cmd.args),
            "cat" => self.cmd_cat(&cmd.args),
            "which" => self.cmd_which(&cmd.args),
            "mem" => self.cmd_mem(),
            "ps" => self.cmd_ps(),
            "time" | "uptime" => self.io.writeln(&format_u64(self.sys.uptime())),
            "launch" | "run" => self.cmd_launch(&cmd.args),
            "reboot" => self.sys.reboot(),
            "jobs" => self.cmd_jobs(),
            "fg" => self.cmd_fg(&cmd.args),
            "bg" => self.cmd_bg(&cmd.args),
            _ => self
                .io
                .writeln(&format!("sigma-sh: {}: command not found", cmd.name)),
        }
    }

    /// Execute a pipeline of commands.
    pub fn execute_pipeline(&mut self, pipeline: &Pipeline) {
        // Note: Full pipeline implementation requires:
        // - Process spawning with pipe creation
        // - Pipe file descriptor management
        // - Connecting stdout of one command to stdin of next
        // - Waiting for all processes in pipeline
        // For now, this is a stub that executes commands sequentially
        for cmd in &pipeline.commands {
            self.io
                .writeln(&format!("sigma-sh: pipeline: executing {}", cmd.name));
            // Execute each command in sequence (no real piping yet)
            self.execute_single_command(cmd);
        }
    }

    /// Execute a single command (helper for pipelines).
    fn execute_single_command(&mut self, cmd: &Command) {
        match cmd.name.as_str() {
            "help" | "?" => self.cmd_help(),
            "exit" | "logout" | "quit" => self.running = false,
            "echo" => self.io.writeln(&cmd.args.join(" ")),
            "pwd" => self.io.writeln(&self.cwd),
            "ls" => self.cmd_ls(&cmd.args),
            "cat" => self.cmd_cat(&cmd.args),
            "which" => self.cmd_which(&cmd.args),
            "mem" => self.cmd_mem(),
            "ps" => self.cmd_ps(),
            "time" | "uptime" => self.io.writeln(&format_u64(self.sys.uptime())),
            "launch" | "run" => self.cmd_launch(&cmd.args),
            "reboot" => self.sys.reboot(),
            "jobs" => self.cmd_jobs(),
            "fg" => self.cmd_fg(&cmd.args),
            "bg" => self.cmd_bg(&cmd.args),
            _ => self
                .io
                .writeln(&format!("sigma-sh: {}: command not found", cmd.name)),
        }
    }

    fn cmd_help(&mut self) {
        for l in [
            "builtins:",
            "  help            show this text",
            "  ls [dir]        list directory",
            "  pwd             print working directory",
            "  echo <text>     print text",
            "  cat <file>      print file",
            "  which <prog>    locate a program on PATH",
            "  mem             memory usage",
            "  ps              process list",
            "  time            uptime seconds",
            "  launch <prog>   run a program",
            "  jobs            list background jobs",
            "  fg <job>        bring job to foreground",
            "  bg <job>        send job to background",
            "  reboot          restart the machine",
            "  exit            leave the shell",
        ] {
            self.io.writeln(l);
        }
    }

    fn resolve(&self, arg: Option<&String>) -> String {
        let Some(a) = arg else {
            return self.cwd.clone();
        };
        if a == "." {
            return self.cwd.clone();
        }
        if a.starts_with('/') {
            return a.clone();
        }
        if self.cwd == "/" {
            format!("/{}", a)
        } else {
            format!("{}/{}", self.cwd, a)
        }
    }

    fn cmd_ls(&mut self, args: &[String]) {
        let path = self.resolve(args.first());
        match self.fs.list(&path) {
            Some(names) => {
                for n in names {
                    self.io.writeln(&n);
                }
            }
            None => self.io.writeln(&format!("ls: {}: no such directory", path)),
        }
    }

    fn cmd_cat(&mut self, args: &[String]) {
        let Some(a) = args.first() else {
            self.io.writeln("cat: missing file");
            return;
        };
        let path = self.resolve(Some(a));
        match self.fs.read_to_string(&path) {
            Some(s) => self.io.write(&s),
            None => self.io.writeln(&format!("cat: {}: no such file", path)),
        }
    }

    /// PATH search dirs for `which`, matching the sigmaos-target layout.
    const WHICH_PATH: [&'static str; 4] = ["/bin", "/sbin", "/usr/bin", "/usr/sbin"];

    fn cmd_which(&mut self, args: &[String]) {
        let Some(name) = args.first() else {
            self.io.writeln("which: missing program name");
            return;
        };
        if name.contains('/') {
            let path = if name.starts_with('/') {
                name.clone()
            } else {
                format!("{}/{}", self.cwd, name)
            };
            let found = match path.rsplit_once('/') {
                Some((dir, base)) => self
                    .fs
                    .list(dir)
                    .is_some_and(|names| names.iter().any(|n| n == base)),
                None => false,
            };
            if found {
                self.io.writeln(&path);
            } else {
                self.io.writeln(&format!("which: no {} in {}", name, path));
            }
            return;
        }
        for dir in Self::WHICH_PATH {
            if let Some(names) = self.fs.list(dir) {
                if names.iter().any(|n| n == name) {
                    self.io.writeln(&format!("{}/{}", dir, name));
                    return;
                }
            }
        }
        self.io.writeln(&format!("which: no {} in PATH", name));
    }

    fn cmd_mem(&mut self) {
        let (total, free) = self.mem.memory_kb();
        self.io
            .writeln(&format!("mem: {} KiB total, {} KiB free", total, free));
    }

    fn cmd_ps(&mut self) {
        for row in self.procs.list() {
            self.io.writeln(&row);
        }
    }

    fn cmd_launch(&mut self, args: &[String]) {
        let Some(prog) = args.first() else {
            self.io.writeln("launch: missing program");
            return;
        };
        match self.sys.launch(prog, &args[1..]) {
            Ok(pid) => self.io.writeln(&format!("launched {} (pid {})", prog, pid)),
            Err(e) => self.io.writeln(&format!("launch: {}: {}", prog, e)),
        }
    }

    /// Job control: list background jobs (stub for now)
    fn cmd_jobs(&mut self) {
        self.io
            .writeln("jobs: no background jobs (job control not implemented)");
    }

    /// Job control: bring job to foreground (stub for now)
    fn cmd_fg(&mut self, _args: &[String]) {
        self.io.writeln("fg: job control not implemented");
    }

    /// Job control: send job to background (stub for now)
    fn cmd_bg(&mut self, _args: &[String]) {
        self.io.writeln("bg: job control not implemented");
    }
}

fn format_u64(v: u64) -> String {
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct TestIo {
        input: Vec<String>,
        output: Rc<RefCell<String>>,
    }
    impl Io for TestIo {
        fn read_line(&mut self) -> Option<String> {
            if self.input.is_empty() {
                None
            } else {
                Some(self.input.remove(0))
            }
        }
        fn write(&mut self, s: &str) {
            self.output.borrow_mut().push_str(s);
        }
    }

    struct TestFs;
    impl Fs for TestFs {
        fn list(&self, path: &str) -> Option<Vec<String>> {
            match path {
                "/" => Some(vec!["bin".to_string(), "etc".to_string()]),
                "/bin" => Some(vec!["sh".to_string(), "init".to_string()]),
                _ => None,
            }
        }
        fn read_to_string(&self, path: &str) -> Option<String> {
            match path {
                "/etc/hostname" => Some("sigmaos\n".to_string()),
                _ => None,
            }
        }
        fn cwd(&self) -> String {
            "/".into()
        }
    }

    struct TestProcs;
    impl ProcTable for TestProcs {
        fn list(&self) -> Vec<String> {
            vec!["1 init".to_string(), "42 sigma-sh".to_string()]
        }
    }

    struct TestMem;
    impl MemInfo for TestMem {
        fn memory_kb(&self) -> (u64, u64) {
            (8 * 1024 * 1024, 4 * 1024 * 1024)
        }
    }

    struct TestSys {
        launched: Rc<RefCell<Vec<String>>>,
    }
    impl System for TestSys {
        fn reboot(&self) {}
        fn launch(&self, path: &str, _args: &[String]) -> Result<i64, String> {
            self.launched.borrow_mut().push(path.to_string());
            Ok(100)
        }
        fn uptime(&self) -> u64 {
            3600
        }
    }

    #[allow(clippy::type_complexity)]
    fn shell(
        lines: &[&str],
    ) -> (
        Shell<TestIo, TestFs, TestProcs, TestMem, TestSys>,
        Rc<RefCell<String>>,
    ) {
        let out = Rc::new(RefCell::new(String::new()));
        let launched = Rc::new(RefCell::new(Vec::new()));
        let sh = Shell::new(
            TestIo {
                input: lines.iter().map(|s| s.to_string()).collect(),
                output: out.clone(),
            },
            TestFs,
            TestProcs,
            TestMem,
            TestSys { launched },
        );
        (sh, out)
    }

    #[test]
    fn parse_simple() {
        let c = parse("ls -la /tmp").unwrap();
        assert_eq!(c.name, "ls");
        assert_eq!(c.args, vec!["-la".to_string(), "/tmp".to_string()]);
    }

    #[test]
    fn parse_quotes() {
        let c = parse("echo \"hello world\"").unwrap();
        assert_eq!(c.args, vec!["hello world".to_string()]);
    }

    #[test]
    fn empty_line_is_none() {
        assert!(parse("").is_none());
        assert!(parse("   ").is_none());
    }

    #[test]
    fn builtins_run() {
        let (mut sh, out) = shell(&["help", "pwd", "ls /bin", "mem", "ps", "time", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("builtins:"));
        assert!(o.contains("sigma> "));
        assert!(o.contains("/"));
        assert!(o.contains("sh"));
        assert!(o.contains("init"));
        assert!(o.contains("8388608")); // 8MiB in KiB
        assert!(o.contains("init"));
        assert!(o.contains("3600"));
    }

    #[test]
    fn unknown_command() {
        let (mut sh, out) = shell(&["frobnicate", "exit"]);
        sh.run();
        assert!(out.borrow().contains("command not found"));
    }

    #[test]
    fn launch_reports() {
        let (mut sh, out) = shell(&["launch /bin/sh arg1", "exit"]);
        sh.run();
        assert!(out.borrow().contains("pid 100"));
    }

    #[test]
    fn which_finds_on_path() {
        let (mut sh, out) = shell(&[
            "which sh",
            "which missing",
            "which /bin/sh",
            "which /nope",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("/bin/sh"));
        assert!(o.contains("which: no missing in PATH"));
        assert!(o.contains("which: no /nope in /nope"));
    }

    #[test]
    fn cat_missing_file() {
        let (mut sh, out) = shell(&["cat /nope", "exit"]);
        sh.run();
        assert!(out.borrow().contains("no such file"));
    }
}
