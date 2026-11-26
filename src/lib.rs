//! SigmaOS sigma-sh — minimal rescue shell.
//!
//! Builtins: help, ls, pwd, cd, echo, cat, which, history, export, unset,
//! mem, ps, time, launch, reboot, true/false/:/clear, plus `*` glob
//! expansion in arguments. `launch` resolves bare names through PATH.
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
    /// Write data to a file, creating it; `append` selects append mode.
    /// Default: unsupported.
    fn write_file(&self, _path: &str, _data: &str, _append: bool) -> bool {
        false
    }
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

/// Split a line on `;` outside double quotes.
pub fn split_semicolons(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in line.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            ';' if !in_quote => {
                let t = cur.trim();
                if !t.is_empty() {
                    parts.push(t.to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        parts.push(t.to_string());
    }
    parts
}

/// Split a line on `&&` outside double quotes.
pub fn split_and(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            '&' if !in_quote && chars.peek() == Some(&'&') => {
                chars.next();
                let t = cur.trim();
                if !t.is_empty() {
                    parts.push(t.to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        parts.push(t.to_string());
    }
    parts
}

/// Normalize `.`/`..` segments in an absolute path.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        let mut p = String::from("/");
        p.push_str(&parts.join("/"));
        p
    }
}

/// Split a line on `||` outside double quotes.
pub fn split_or(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            '|' if !in_quote && chars.peek() == Some(&'|') => {
                chars.next();
                let t = cur.trim();
                if !t.is_empty() {
                    parts.push(t.to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        parts.push(t.to_string());
    }
    parts
}

/// The shell.
pub struct Shell<I, F, P, M, S> {
    pub io: I,
    pub fs: F,
    pub procs: P,
    pub mem: M,
    pub sys: S,
    pub cwd: String,
    pub oldpwd: Option<String>,
    pub history: Vec<String>,
    pub env: Vec<(String, String)>,
    pub last_ok: bool,
    /// Active `>`/`>>` redirect target (path, append) for the current command.
    pub redirect: Option<(String, bool)>,
    /// Whether the current `>` target has been created/truncated already.
    pub redirect_initialized: bool,
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
            oldpwd: None,
            history: Vec::new(),
            env: Vec::new(),
            last_ok: true,
            redirect: None,
            redirect_initialized: false,
            running: true,
        }
    }

    /// Run the REPL until EOF or `exit`.
    pub fn run(&mut self) {
        self.emit(&format!(
            "sigma-sh {} — type 'help'",
            env!("CARGO_PKG_VERSION")
        ));
        while self.running {
            self.io.write("sigma> ");
            let Some(line) = self.io.read_line() else {
                break;
            };
            if self.history.len() >= 100 {
                self.history.remove(0);
            }
            self.history.push(line.clone());
            self.execute(&line);
        }
    }

    /// Execute one command line.
    pub fn execute(&mut self, line: &str) {
        // `;` separates commands (quote-aware).
        let parts = split_semicolons(line);
        if parts.len() > 1 {
            for part in parts {
                if self.running {
                    self.execute(&part);
                }
            }
            return;
        }
        // `||` runs the next command only if the previous failed.
        let or_parts = split_or(line);
        if or_parts.len() > 1 {
            let mut first = true;
            for part in or_parts {
                if !self.running {
                    break;
                }
                if first || !self.last_ok {
                    self.execute(&part);
                }
                first = false;
            }
            return;
        }
        // `&&` runs the next command only if the previous succeeded.
        let and_parts = split_and(line);
        if and_parts.len() > 1 {
            let mut first = true;
            for part in and_parts {
                if !self.running {
                    break;
                }
                if first || self.last_ok {
                    self.execute(&part);
                }
                first = false;
            }
            return;
        }
        // `|` pipelines (single `|` only; `||` was handled above).
        if line.contains('|') {
            if let Some(pipeline) = parse_pipeline(line) {
                self.execute_pipeline(&pipeline);
                return;
            }
        }

        // Fall back to single command
        let Some(cmd) = parse(line) else {
            return;
        };
        let cmd = self.expand_command(cmd);
        self.execute_single_command(&cmd);
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
            self.emit(&format!("sigma-sh: pipeline: executing {}", cmd.name));
            // Execute each command in sequence (no real piping yet)
            let expanded = self.expand_command(cmd.clone());
            self.execute_single_command(&expanded);
        }
    }

    /// Write raw output honoring an active `>`/`>>` redirect.
    fn emit_raw(&mut self, s: &str) {
        match &self.redirect {
            Some((path, append)) => {
                // First write to a `>` target truncates; later writes append.
                let app = *append || self.redirect_initialized;
                if self.fs.write_file(path, s, app) {
                    self.redirect_initialized = true;
                }
            }
            None => self.io.write(s),
        }
    }

    /// Write a line of output honoring an active redirect.
    fn emit(&mut self, s: &str) {
        self.emit_raw(&format!("{}\n", s));
    }

    /// Expand `$VAR` and `${VAR}` in command args using the shell env.
    fn expand(&self, s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '$' {
                out.push(c);
                continue;
            }
            let name = if chars.peek() == Some(&'{') {
                chars.next();
                let mut n = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    n.push(c);
                }
                n
            } else {
                let mut n = String::new();
                while chars
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    n.push(chars.next().unwrap());
                }
                n
            };
            if name.is_empty() {
                out.push('$');
                continue;
            }
            if let Some((_, v)) = self.env.iter().find(|(k, _)| *k == name) {
                out.push_str(v);
            }
        }
        out
    }

    fn expand_command(&self, cmd: Command) -> Command {
        let mut args = Vec::new();
        for a in &cmd.args {
            let expanded = self.expand(a);
            if expanded.contains('*') {
                args.extend(self.glob_expand(&expanded));
            } else {
                args.push(expanded);
            }
        }
        Command {
            name: cmd.name,
            args,
        }
    }

    /// Expand a `*` glob against the filesystem; unmatched patterns pass through.
    fn glob_expand(&self, pat: &str) -> Vec<String> {
        let (dir_raw, base) = match pat.rfind('/') {
            Some(i) => (&pat[..=i], &pat[i + 1..]),
            None => ("", pat),
        };
        let dir = if dir_raw.is_empty() {
            format!("{}/", self.cwd)
        } else if dir_raw.starts_with('/') {
            dir_raw.to_string()
        } else {
            format!("{}/{}", self.cwd, dir_raw)
        };
        let dir = normalize(dir.trim_end_matches('/'));
        let prefix = if dir == "/" {
            String::from("/")
        } else {
            format!("{}/", dir)
        };
        let Some(names) = self.fs.list(&dir) else {
            return alloc::vec![pat.to_string()];
        };
        let mut matches: Vec<String> = names
            .iter()
            .filter(|n| glob_match(base, n))
            .map(|n| format!("{}{}", prefix, n))
            .collect();
        if matches.is_empty() {
            return alloc::vec![pat.to_string()];
        }
        matches.sort();
        matches
    }

    /// Execute a single command (helper for pipelines). Handles `>`/`>>`.
    fn execute_single_command(&mut self, cmd: &Command) {
        // `>`/`>>` redirection: last occurrence wins, target resolved vs cwd.
        let mut args = cmd.args.clone();
        let mut redir = None;
        if let Some(i) = args.iter().position(|a| a == ">" || a == ">>") {
            let append = args[i] == ">>";
            if i + 1 < args.len() {
                redir = Some((self.resolve(Some(&args[i + 1])), append));
                args.drain(i..=i + 1);
            } else {
                args.remove(i);
            }
        }
        let prev_redir = self.redirect.take();
        if let Some(r) = redir {
            self.redirect = Some(r);
            self.redirect_initialized = false;
        }
        self.last_ok = is_builtin(&cmd.name);
        match cmd.name.as_str() {
            "help" | "?" => self.cmd_help(),
            "exit" | "logout" | "quit" => self.running = false,
            "true" | ":" => {}
            "false" => self.last_ok = false,
            "clear" => self.io.write("\x1b[2J\x1b[H"),
            "echo" => self.emit(&args.join(" ")),
            "pwd" => {
                let c = self.cwd.clone();
                self.emit(&c);
            }
            "cd" => self.cmd_cd(&args),
            "ls" => self.cmd_ls(&args),
            "cat" => self.cmd_cat(&args),
            "which" => self.cmd_which(&args),
            "history" => self.cmd_history(),
            "export" => self.cmd_export(&args),
            "unset" => self.cmd_unset(&args),
            "mem" => self.cmd_mem(),
            "ps" => self.cmd_ps(),
            "time" | "uptime" => self.emit(&format_u64(self.sys.uptime())),
            "launch" | "run" => self.cmd_launch(&args),
            "reboot" => self.sys.reboot(),
            "jobs" => self.cmd_jobs(),
            "fg" => self.cmd_fg(&args),
            "bg" => self.cmd_bg(&args),
            _ => self.emit(&format!("sigma-sh: {}: command not found", cmd.name)),
        }
        if is_builtin(&cmd.name) && cmd.name != "false" {
            self.last_ok = true;
        }
        self.redirect = prev_redir;
    }

    fn cmd_help(&mut self) {
        for l in [
            "builtins:",
            "  help            show this text",
            "  ls [dir]        list directory",
            "  pwd             print working directory",
            "  cd [dir]        change directory",
            "  echo <text>     print text",
            "  cat <file>      print file",
            "  which <prog>    locate a program on PATH",
            "  history         list command history",
            "  export V=val    set an environment variable",
            "  unset V         remove an environment variable",
            "  true/:/false    no-op / fail (for && and ||)",
            "  clear           clear the screen",
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
            self.emit(l);
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

    fn cmd_cd(&mut self, args: &[String]) {
        let target = match args.first() {
            None => "/".to_string(),
            Some(a) if a == "~" => "/".to_string(),
            Some(a) if a == "-" => match self.oldpwd.clone() {
                Some(prev) => prev,
                None => {
                    self.emit("cd: OLDPWD not set");
                    return;
                }
            },
            Some(a) => self.resolve(Some(a)),
        };
        let norm = normalize(&target);
        let is_back = args.first().is_some_and(|a| a == "-");
        match self.fs.list(&norm) {
            Some(_) => {
                self.oldpwd = Some(core::mem::replace(&mut self.cwd, norm.clone()));
                if is_back {
                    self.emit(&norm);
                }
            }
            None => self.emit(&format!("cd: {}: no such directory", target)),
        }
    }

    fn cmd_ls(&mut self, args: &[String]) {
        let path = self.resolve(args.first());
        match self.fs.list(&path) {
            Some(names) => {
                for n in names {
                    self.emit(&n);
                }
            }
            None => self.emit(&format!("ls: {}: no such directory", path)),
        }
    }

    fn cmd_cat(&mut self, args: &[String]) {
        let Some(a) = args.first() else {
            self.emit("cat: missing file");
            return;
        };
        let path = self.resolve(Some(a));
        match self.fs.read_to_string(&path) {
            Some(s) => self.emit_raw(&s),
            None => self.emit(&format!("cat: {}: no such file", path)),
        }
    }

    /// PATH search dirs for `which`, matching the sigmaos-target layout.
    const WHICH_PATH: [&'static str; 4] = ["/bin", "/sbin", "/usr/bin", "/usr/sbin"];

    fn cmd_which(&mut self, args: &[String]) {
        let Some(name) = args.first() else {
            self.emit("which: missing program name");
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
                self.emit(&path);
            } else {
                self.emit(&format!("which: no {} in {}", name, path));
            }
            return;
        }
        for dir in Self::WHICH_PATH {
            if let Some(names) = self.fs.list(dir) {
                if names.iter().any(|n| n == name) {
                    self.emit(&format!("{}/{}", dir, name));
                    return;
                }
            }
        }
        self.emit(&format!("which: no {} in PATH", name));
    }

    fn cmd_history(&mut self) {
        let lines: Vec<String> = self
            .history
            .iter()
            .enumerate()
            .map(|(i, h)| format!("{:4}  {}", i + 1, h))
            .collect();
        for l in lines {
            self.emit(&l);
        }
    }

    fn cmd_export(&mut self, args: &[String]) {
        if args.is_empty() {
            let lines: Vec<String> = self
                .env
                .iter()
                .map(|(k, v)| format!("export {}={}", k, v))
                .collect();
            for l in lines {
                self.emit(&l);
            }
            return;
        }
        for a in args {
            match a.split_once('=') {
                Some((k, v)) if !k.is_empty() => {
                    if let Some(e) = self.env.iter_mut().find(|(ek, _)| ek == k) {
                        e.1 = v.to_string();
                    } else {
                        self.env.push((k.to_string(), v.to_string()));
                    }
                }
                _ => self.emit(&format!("export: {}: not a valid assignment", a)),
            }
        }
    }

    fn cmd_unset(&mut self, args: &[String]) {
        for a in args {
            self.env.retain(|(k, _)| k != a);
        }
    }

    fn cmd_mem(&mut self) {
        let (total, free) = self.mem.memory_kb();
        self.emit(&format!("mem: {} KiB total, {} KiB free", total, free));
    }

    fn cmd_ps(&mut self) {
        for row in self.procs.list() {
            self.emit(&row);
        }
    }

    fn cmd_launch(&mut self, args: &[String]) {
        let Some(prog) = args.first() else {
            self.emit("launch: missing program");
            return;
        };
        // Bare names resolve through PATH dirs, like a real shell.
        let resolved;
        let prog = if prog.contains('/') {
            prog.as_str()
        } else {
            resolved = Self::WHICH_PATH
                .iter()
                .find_map(|dir| {
                    let p = format!("{}/{}", dir, prog);
                    let (d, b) = p.rsplit_once('/')?;
                    self.fs
                        .list(d)
                        .is_some_and(|names| names.iter().any(|n| n == b))
                        .then_some(p)
                })
                .unwrap_or_else(|| prog.clone());
            resolved.as_str()
        };
        match self.sys.launch(prog, &args[1..]) {
            Ok(pid) => self.emit(&format!("launched {} (pid {})", prog, pid)),
            Err(e) => self.emit(&format!("launch: {}: {}", prog, e)),
        }
    }

    /// Job control: list background jobs (stub for now)
    fn cmd_jobs(&mut self) {
        self.emit("jobs: no background jobs (job control not implemented)");
    }

    /// Job control: bring job to foreground (stub for now)
    fn cmd_fg(&mut self, _args: &[String]) {
        self.emit("fg: job control not implemented");
    }

    /// Job control: send job to background (stub for now)
    fn cmd_bg(&mut self, _args: &[String]) {
        self.emit("bg: job control not implemented");
    }
}

/// Match a glob pattern (`*` segments must appear in order).
fn glob_match(pat: &str, name: &str) -> bool {
    let segs: Vec<&str> = pat.split('*').collect();
    if segs.len() == 1 {
        return pat == name;
    }
    let mut rest = name;
    for (i, seg) in segs.iter().enumerate() {
        if seg.is_empty() {
            continue;
        }
        match rest.find(seg) {
            Some(pos) => {
                if i == 0 && pos != 0 {
                    return false;
                }
                rest = &rest[pos + seg.len()..];
            }
            None => return false,
        }
    }
    if let Some(last) = segs.last() {
        if !last.is_empty() && !name.ends_with(last) {
            return false;
        }
    }
    true
}

/// Names dispatched by the shell (used for `&&` success tracking).
fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "help"
            | "?"
            | "exit"
            | "logout"
            | "quit"
            | "true"
            | ":"
            | "false"
            | "clear"
            | "echo"
            | "pwd"
            | "cd"
            | "ls"
            | "cat"
            | "which"
            | "history"
            | "export"
            | "unset"
            | "mem"
            | "ps"
            | "time"
            | "uptime"
            | "launch"
            | "run"
            | "reboot"
            | "jobs"
            | "fg"
            | "bg"
    )
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

    #[derive(Default)]
    struct TestFs {
        writes: Rc<RefCell<Vec<(String, String, bool)>>>,
    }
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
        fn write_file(&self, path: &str, data: &str, append: bool) -> bool {
            self.writes
                .borrow_mut()
                .push((path.to_string(), data.to_string(), append));
            true
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
            TestFs::default(),
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
    fn cd_changes_directory() {
        let (mut sh, out) = shell(&["cd /bin", "pwd", "cd ..", "pwd", "cd /nope", "pwd", "exit"]);
        sh.run();
        let o = out.borrow();
        // cd /bin then pwd -> /bin appears; cd .. returns to /; cd /nope fails.
        assert!(o.contains("/bin"));
        assert!(o.contains("cd: /nope: no such directory"));
    }

    #[test]
    fn cd_dash_returns_to_oldpwd() {
        let (mut sh, out) = shell(&["cd -", "cd /bin", "pwd", "cd -", "pwd", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("cd: OLDPWD not set"));
        assert!(o.contains("/bin"));
    }

    #[test]
    fn semicolons_separate_commands() {
        let (mut sh, out) = shell(&["echo one; echo two", "echo \"a;b\"", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("one"));
        assert!(o.contains("two"));
        assert!(o.contains("a;b"));
    }

    #[test]
    fn and_and_skips_after_failure() {
        let (mut sh, out) = shell(&[
            "nosuchcmd && echo should_not_print",
            "echo ok && echo yes",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(!o.contains("should_not_print"));
        assert!(o.contains("yes"));
    }

    #[test]
    fn or_or_runs_only_after_failure() {
        let (mut sh, out) = shell(&[
            "nosuchcmd || echo fallback",
            "echo ok || echo skipped",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("fallback"));
        assert!(!o.contains("skipped"));
    }

    #[test]
    fn true_false_drive_and_or() {
        let (mut sh, out) = shell(&[
            "true && echo yep",
            "false && echo nope",
            "false || echo fallback",
            ": && echo colon-ok",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("yep"));
        assert!(!o.contains("nope"));
        assert!(o.contains("fallback"));
        assert!(o.contains("colon-ok"));
    }

    #[test]
    fn launch_resolves_path() {
        let (mut sh, out) = shell(&["launch sh arg1", "exit"]);
        sh.run();
        let launched = sh.sys.launched.borrow();
        assert_eq!(launched.as_slice(), &["/bin/sh".to_string()]);
        assert!(out.borrow().contains("launched /bin/sh (pid 100)"));
    }

    #[test]
    fn redirect_writes_file() {
        let (mut sh, out) = shell(&[
            "echo hello > /tmp/out",
            "echo world >> /tmp/out",
            "pwd > /tmp/pwd",
            "exit",
        ]);
        sh.run();
        // Redirected output must not hit stdout.
        assert!(!out.borrow().contains("hello"));
        let writes = sh.fs.writes.borrow();
        // `>` truncates on first write, `>>` always appends.
        assert!(writes.contains(&("/tmp/out".to_string(), "hello\n".to_string(), false)));
        assert!(writes.contains(&("/tmp/out".to_string(), "world\n".to_string(), true)));
        assert!(writes.contains(&("/tmp/pwd".to_string(), "/\n".to_string(), false)));
    }

    #[test]
    fn history_lists_commands() {
        let (mut sh, out) = shell(&["echo hi", "pwd", "history", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("echo hi"));
        assert!(o.contains("pwd"));
    }

    #[test]
    fn export_and_expand() {
        let (mut sh, out) = shell(&[
            "export FOO=bar",
            "echo $FOO",
            "echo ${FOO}-baz",
            "unset FOO",
            "echo [$FOO]",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("bar"));
        assert!(o.contains("bar-baz"));
        assert!(o.contains("[]"));
    }

    #[test]
    fn glob_expands() {
        let (mut sh, out) = shell(&["echo /b*", "echo /*", "echo /zzz*", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("/bin"));
        assert!(o.contains("/bin /etc"));
        assert!(o.contains("/zzz*")); // unmatched pattern passes through
    }

    #[test]
    fn cat_missing_file() {
        let (mut sh, out) = shell(&["cat /nope", "exit"]);
        sh.run();
        assert!(out.borrow().contains("no such file"));
    }
}
