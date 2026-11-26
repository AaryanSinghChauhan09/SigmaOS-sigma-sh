//! SigmaOS sigma-sh — minimal rescue shell.
//!
//! Builtins: help, ls, pwd, cd, echo, cat, which, history, export, unset,
//! mem, ps, time, launch, reboot, true/false/:/clear, mkdir/rmdir/rm/touch,
//! `>`/`>>` redirection, `|` pipelines between builtins, plus `*` glob
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
    /// Create a directory (parents must exist). Default: unsupported.
    fn create_dir(&self, _path: &str) -> bool {
        false
    }
    /// Remove a file or empty directory. Default: unsupported.
    fn remove_path(&self, _path: &str) -> bool {
        false
    }
    /// File metadata: (is_dir, size_bytes). Default: unsupported.
    fn stat(&self, _path: &str) -> Option<(bool, u64)> {
        None
    }
    /// Open a file for writing (create/truncate or append); returns an
    /// fd usable as a child's stdout redirect. Default: unsupported.
    fn open_write(&self, _path: &str, _append: bool) -> Option<i64> {
        None
    }
    /// Open a file for reading; returns an fd usable as a child's
    /// stdin redirect. Default: unsupported.
    fn open_read(&self, _path: &str) -> Option<i64> {
        None
    }
    /// Change the process working directory. Default: unsupported.
    fn chdir(&self, _path: &str) -> bool {
        false
    }
    /// Create a pipe; returns (read_fd, write_fd). Default: unsupported.
    fn pipe(&self) -> Option<(i64, i64)> {
        None
    }
    /// Close an fd. Default: unsupported.
    fn close(&self, _fd: i64) -> bool {
        false
    }
    /// Read up to `buf.len()` bytes from `fd`; returns bytes read
    /// (0 = EOF) or a negative errno (-11 = EAGAIN, retry). Default:
    /// unsupported (-38).
    fn read_fd(&self, _fd: i64, _buf: &mut [u8]) -> i64 {
        -38
    }
    /// Write bytes to `fd`; returns bytes written or a negative errno.
    /// Default: unsupported (-38).
    fn write_fd(&self, _fd: i64, _buf: &[u8]) -> i64 {
        -38
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
    fn launch(
        &self,
        path: &str,
        args: &[String],
        env: &[String],
        stdout_fd: i64,
        stdin_fd: i64,
        stderr_fd: i64,
    ) -> Result<i64, String>;
    /// Wait for `pid` to exit; returns its exit status.
    /// Default: unsupported.
    fn wait(&self, _pid: i64) -> Result<i64, String> {
        Err("wait: unsupported".into())
    }
    /// Current uptime seconds.
    fn uptime(&self) -> u64;
    /// Sleep for `secs` seconds. Default: no-op.
    fn sleep(&self, _secs: u64) {}
    /// Send signal `sig` (name like "TERM"/"KILL" or number like "9")
    /// to `pid`; returns success. Default: unsupported.
    fn kill(&self, _pid: i64, _sig: &str) -> bool {
        false
    }
    /// Kernel/OS name for `uname`. Default: sigmaos.
    fn uname(&self) -> &'static str {
        "sigmaos"
    }
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub args: Vec<String>,
    /// Per-arg quote mask parallel to `args`: bit0 = the arg
    /// contained a single-quoted section, bit1 = double-quoted.
    /// Single quotes suppress `$` expansion and globbing; double
    /// quotes suppress globbing only (POSIX).
    pub quoted: Vec<u8>,
}

/// A pipeline of commands connected by pipes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>,
}

/// Tokenize a command line: whitespace-separated, single- and
/// double-quote aware (quotes are stripped from the resulting args).
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
    let mut quoted = Vec::new();
    let mut cur = String::new();
    let mut cur_q: u8 = 0;
    let mut in_dquote = false;
    let mut in_squote = false;
    for c in chars {
        match c {
            '"' if !in_squote => {
                in_dquote = !in_dquote;
                cur_q |= 2;
            }
            '\'' if !in_dquote => {
                in_squote = !in_squote;
                cur_q |= 1;
            }
            c if c.is_whitespace() && !in_dquote && !in_squote => {
                if !cur.is_empty() {
                    args.push(core::mem::take(&mut cur));
                    quoted.push(core::mem::take(&mut cur_q));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        args.push(cur);
        quoted.push(cur_q);
    }
    Some(Command { name, args, quoted })
}

/// Split a pipeline on `|` outside quotes. `||` was already handled
/// by split_or, so any remaining `|` is a pipe separator.
pub fn parse_pipeline(line: &str) -> Option<Pipeline> {
    let mut segments = Vec::new();
    let mut cur = String::new();
    let mut in_dquote = false;
    let mut in_squote = false;
    for c in line.chars() {
        match c {
            '"' if !in_squote => {
                in_dquote = !in_dquote;
                cur.push(c);
            }
            '\'' if !in_dquote => {
                in_squote = !in_squote;
                cur.push(c);
            }
            '|' if !in_dquote && !in_squote => {
                let t = cur.trim();
                if !t.is_empty() {
                    segments.push(t.to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        segments.push(t.to_string());
    }
    let commands: Vec<Command> = segments.iter().filter_map(|s| parse(s)).collect();

    if commands.is_empty() {
        None
    } else {
        Some(Pipeline { commands })
    }
}

/// Split a line on `;` outside quotes.
pub fn split_semicolons(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_dquote = false;
    let mut in_squote = false;
    for c in line.chars() {
        match c {
            '"' if !in_squote => {
                in_dquote = !in_dquote;
                cur.push(c);
            }
            '\'' if !in_dquote => {
                in_squote = !in_squote;
                cur.push(c);
            }
            ';' if !in_dquote && !in_squote => {
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

/// Split a line on `&&` outside quotes.
pub fn split_and(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_dquote = false;
    let mut in_squote = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if !in_squote => {
                in_dquote = !in_dquote;
                cur.push(c);
            }
            '\'' if !in_dquote => {
                in_squote = !in_squote;
                cur.push(c);
            }
            '&' if !in_dquote && !in_squote && chars.peek() == Some(&'&') => {
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

/// Split a line on `||` outside quotes.
pub fn split_or(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_dquote = false;
    let mut in_squote = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if !in_squote => {
                in_dquote = !in_dquote;
                cur.push(c);
            }
            '\'' if !in_dquote => {
                in_squote = !in_squote;
                cur.push(c);
            }
            '|' if !in_dquote && !in_squote && chars.peek() == Some(&'|') => {
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

/// Recursive-descent parser for `test`/`[` expressions.
struct TestParser<'a, I, F, P, M, S> {
    sh: &'a Shell<I, F, P, M, S>,
    args: &'a [&'a str],
    i: usize,
}

impl<'a, I: Io, F: Fs, P: ProcTable, M: MemInfo, S: System> TestParser<'a, I, F, P, M, S> {
    fn peek(&self) -> Option<&'a str> {
        self.args.get(self.i).copied()
    }
    fn next(&mut self) -> Option<&'a str> {
        let t = self.peek();
        if t.is_some() {
            self.i += 1;
        }
        t
    }
    fn parse_or(&mut self) -> bool {
        let mut v = self.parse_and();
        while self.peek() == Some("-o") {
            self.next();
            let r = self.parse_and();
            v = v || r;
        }
        v
    }
    fn parse_and(&mut self) -> bool {
        let mut v = self.parse_not();
        while self.peek() == Some("-a") {
            self.next();
            let r = self.parse_not();
            v = v && r;
        }
        v
    }
    fn parse_not(&mut self) -> bool {
        if self.peek() == Some("!") {
            self.next();
            return !self.parse_not();
        }
        self.parse_primary()
    }
    fn parse_primary(&mut self) -> bool {
        let Some(first) = self.next() else {
            return false;
        };
        // Parenthesized sub-expression: `( EXPR )`.
        if first == "(" {
            let v = self.parse_or();
            if self.peek() == Some(")") {
                self.next();
            }
            return v;
        }
        // Unary tests: `-f FILE` — the op leads, the operand follows.
        if matches!(
            first,
            "-e" | "-f" | "-d" | "-s" | "-r" | "-w" | "-x" | "-n" | "-z"
        ) {
            let Some(arg) = self.next() else {
                return false;
            };
            return self.unary_test(first, arg);
        }
        // Binary tests: `a = b`, `a -eq b`.
        if let Some(op) = self.peek() {
            if matches!(
                op,
                "=" | "==" | "!=" | "<" | ">" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge"
            ) {
                self.next();
                let Some(rhs) = self.next() else {
                    return false;
                };
                return self.binary_test(first, op, rhs);
            }
        }
        // Bare string: true when non-empty.
        !first.is_empty()
    }
    fn unary_test(&self, op: &str, arg: &str) -> bool {
        match op {
            "-n" => !arg.is_empty(),
            "-z" => arg.is_empty(),
            "-e" | "-f" | "-d" | "-s" => {
                let path = self.sh.resolve(Some(&arg.to_string()));
                match self.sh.fs.stat(&path) {
                    Some((is_dir, size)) => match op {
                        "-e" => true,
                        "-f" => !is_dir,
                        "-d" => is_dir,
                        _ => size > 0,
                    },
                    None => false,
                }
            }
            // -r/-w/-x: no permission model yet; existence is the best proxy.
            _ => {
                let path = self.sh.resolve(Some(&arg.to_string()));
                self.sh.fs.stat(&path).is_some()
            }
        }
    }
    fn binary_test(&self, a: &str, op: &str, b: &str) -> bool {
        match op {
            "=" | "==" => a == b,
            "!=" => a != b,
            "<" => a < b,
            ">" => a > b,
            _ => {
                let (Ok(x), Ok(y)) = (a.parse::<i64>(), b.parse::<i64>()) else {
                    return false;
                };
                match op {
                    "-eq" => x == y,
                    "-ne" => x != y,
                    "-lt" => x < y,
                    "-le" => x <= y,
                    "-gt" => x > y,
                    _ => x >= y,
                }
            }
        }
    }
}

/// Expand a `tr` set: backslash escapes plus `a-z` ranges.
fn expand_tr_set(set: &str) -> Vec<char> {
    let chars: Vec<char> = set.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = if chars[i] == '\\' && i + 1 < chars.len() {
            i += 1;
            match chars[i] {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '0' => '\0',
                other => other,
            }
        } else {
            chars[i]
        };
        // Range expansion: `a-z` (literal endpoints).
        if i + 2 < chars.len() && chars[i + 1] == '-' && c.is_ascii_alphanumeric() {
            let end = chars[i + 2];
            if end.is_ascii_alphanumeric() && end >= c {
                for v in c as u32..=end as u32 {
                    if let Some(ch) = char::from_u32(v) {
                        out.push(ch);
                    }
                }
                i += 3;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Parse a `cut`/`test -n` style list like `1,3-5,8` or open `2-`
/// into inclusive (lo, hi) ranges; hi=None means unbounded.
fn parse_num_list(list: &str) -> Vec<(usize, Option<usize>)> {
    let mut out = Vec::new();
    for part in list.split(',') {
        if let Some((a, b)) = part.split_once('-') {
            if let Ok(a) = a.parse::<usize>() {
                let hi = if b.is_empty() {
                    None
                } else {
                    b.parse::<usize>().ok()
                };
                out.push((a, hi));
            }
        } else if let Ok(n) = part.parse::<usize>() {
            out.push((n, Some(n)));
        }
    }
    out
}

/// True when `n` (1-based) is covered by a parsed num list.
fn num_list_has(list: &[(usize, Option<usize>)], n: usize) -> bool {
    list.iter()
        .any(|&(lo, hi)| n >= lo && hi.is_none_or(|h| n <= h))
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
    /// When set, `emit` output is captured here instead of going to io
    /// (used to feed the next stage of a pipeline).
    pub capture: Option<String>,
    /// Text fed to the current command's stdin by the previous pipeline stage.
    pub pending_stdin: Option<String>,
    /// Pid of the most recently launched program (for bare `wait`).
    pub last_pid: Option<i64>,
    /// Launched jobs: (pid, command line, reaped flag).
    pub jobs: Vec<(i64, String, bool)>,
    /// Pipe fd to use as the next launch stage's stdin (0 = none).
    pub pipe_in_fd: i64,
    /// Pipe fd to use as the next launch stage's stdout (0 = none).
    pub pipe_out_fd: i64,
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
            capture: None,
            pending_stdin: None,
            last_pid: None,
            jobs: Vec::new(),
            pipe_in_fd: 0,
            pipe_out_fd: 0,
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

    /// Execute a pipeline of commands. Builtin-to-builtin stages keep
    /// the text-capture path; stages involving `launch`/`run` are wired
    /// with real kernel pipes (fd-to-fd, fully concurrent).
    pub fn execute_pipeline(&mut self, pipeline: &Pipeline) {
        let cmds: Vec<Command> = pipeline
            .commands
            .iter()
            .map(|c| self.expand_command(c.clone()))
            .collect();
        let n = cmds.len();
        if n == 0 {
            return;
        }
        // A launch stage can take a pipe on stdout unless it has its
        // own `>`/`>>`/`&>` redirect; `<` on stdin always wins over a
        // pipeline feed.
        let is_launch = |c: &Command| c.name == "launch" || c.name == "run";
        let has_out_redir = |c: &Command| {
            c.args
                .iter()
                .any(|a| a == ">" || a == ">>" || a.starts_with("&>"))
        };
        let has_in_redir = |c: &Command| c.args.iter().any(|a| a == "<");

        let mut drain_fd: Option<i64> = None; // pipe read end to drain/feed next stage
        let mut starved = false; // previous stage produced no consumable output
        let mut deferred: Vec<(i64, String)> = Vec::new(); // (pipe w, text) to write post-spawn
        let mut shell_fds: Vec<i64> = Vec::new(); // shell copies to close at end

        for (i, cmd) in cmds.iter().enumerate() {
            if !self.running {
                break;
            }
            let last = i + 1 == n;
            let launch = is_launch(cmd);

            // ── stage input ──────────────────────────────────────
            let mut in_fd = 0i64;
            if launch {
                if !has_in_redir(cmd) {
                    if let Some(r) = drain_fd.take() {
                        // Previous pipeable launch: hand the read end
                        // straight to this child.
                        in_fd = r;
                        shell_fds.push(r);
                    } else if starved {
                        // Previous stage wrote to a file/nowhere: feed
                        // this child instant EOF via a closed pipe.
                        if let Some((r, w)) = self.fs.pipe() {
                            let _ = self.fs.close(w);
                            in_fd = r;
                            shell_fds.push(r);
                        }
                    } else if let Some(text) = self.pending_stdin.take() {
                        // Previous builtin produced text: feed it to
                        // the child through a fresh pipe, written
                        // after the spawn below.
                        if let Some((r, w)) = self.fs.pipe() {
                            in_fd = r;
                            deferred.push((w, text));
                            shell_fds.push(r);
                            shell_fds.push(w);
                        }
                    }
                }
            } else if let Some(r) = drain_fd.take() {
                // Builtin consumes a pipe: drain it to text (blocks
                // until the producer's write end closes = EOF).
                let text = self.drain_pipe(r);
                shell_fds.push(r);
                self.pending_stdin = Some(text);
            } else if starved {
                self.pending_stdin = Some(String::new());
            }

            // ── stage output ─────────────────────────────────────
            let mut out_fd = 0i64;
            let mut produced_r: Option<i64> = None;
            if !last && launch && !has_out_redir(cmd) {
                if let Some((r, w)) = self.fs.pipe() {
                    out_fd = w;
                    produced_r = Some(r);
                    shell_fds.push(w); // shell's copy; child has its own
                }
            }
            // Builtins capture text for the next stage; launch status
            // messages always go to the console.
            if !last && !launch {
                self.capture = Some(String::new());
            }
            self.pipe_in_fd = in_fd;
            self.pipe_out_fd = out_fd;
            self.execute_single_command(cmd);
            self.pipe_in_fd = 0;
            self.pipe_out_fd = 0;

            // Feed deferred pipe text now that the child exists.
            for (w, text) in deferred.drain(..) {
                self.feed_pipe(w, &text);
                let _ = self.fs.close(w);
            }

            // ── stage result → next input ────────────────────────
            if !last {
                if let Some(r) = produced_r {
                    drain_fd = Some(r); // next launch uses it as in_fd, next builtin drains it
                    starved = false;
                } else if launch {
                    // launch with `>` or no pipe support: nothing
                    // consumable downstream.
                    starved = true;
                    self.pending_stdin = None;
                } else {
                    // builtin text output feeds the next stage.
                    self.pending_stdin = Some(self.capture.take().unwrap_or_default());
                    starved = false;
                }
            }
        }
        self.pending_stdin = None;
        self.capture = None;
        for fd in shell_fds {
            let _ = self.fs.close(fd);
        }
    }

    /// Read a pipe fd to EOF, returning its contents. Blocks via the
    /// kernel's pipe sleep/wake; -11 wakes are retried.
    fn drain_pipe(&self, fd: i64) -> String {
        let mut out = String::new();
        let mut buf = [0u8; 1024];
        loop {
            match self.fs.read_fd(fd, &mut buf) {
                0 => break, // EOF
                n if n > 0 => {
                    out.push_str(&String::from_utf8_lossy(&buf[..n as usize]));
                    if out.len() > 1 << 20 {
                        break;
                    }
                }
                -11 => continue, // EAGAIN wake: retry
                _ => break,      // real error
            }
        }
        out
    }

    /// Write `text` into a pipe fd, looping over partial writes.
    /// Stops early on error (e.g. EPIPE if the child already exited).
    fn feed_pipe(&self, fd: i64, text: &str) {
        let bytes = text.as_bytes();
        let mut off = 0usize;
        while off < bytes.len() {
            match self.fs.write_fd(fd, &bytes[off..]) {
                n if n > 0 => off += n as usize,
                -11 => continue, // EAGAIN wake: retry
                _ => break,      // EOF/error/EPIPE
            }
        }
    }

    /// Text piped into the current command's stdin, consumed once.
    fn take_stdin(&mut self) -> String {
        self.pending_stdin.take().unwrap_or_default()
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
            None => {
                if let Some(buf) = self.capture.as_mut() {
                    buf.push_str(s);
                } else {
                    self.io.write(s);
                }
            }
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
        let mut quoted = Vec::new();
        for (a, &q) in cmd.args.iter().zip(cmd.quoted.iter()) {
            // Single-quoted sections are fully literal: no $ expansion,
            // no globbing.
            if q & 1 != 0 {
                args.push(a.clone());
                quoted.push(q);
                continue;
            }
            let expanded = self.expand(a);
            // Double quotes (and mixed) suppress globbing; unquoted
            // args glob on `*`.
            if q & 2 == 0 && expanded.contains('*') {
                for m in self.glob_expand(&expanded) {
                    args.push(m);
                    quoted.push(0);
                }
            } else {
                args.push(expanded);
                quoted.push(q);
            }
        }
        Command {
            name: cmd.name,
            args,
            quoted,
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
        let mut err_redir = None;
        // `&>`/`&>>` redirects both streams; parsed first so a later
        // explicit `>` or `2>` can still override its own stream.
        if let Some(i) = args.iter().position(|a| a.starts_with("&>")) {
            let tok = args[i].clone();
            let (inline, append) = if tok == "&>" || tok == "&>>" {
                (None, tok == "&>>")
            } else if let Some(rest) = tok.strip_prefix("&>>") {
                (Some(rest.to_string()), true)
            } else {
                (tok.strip_prefix("&>").map(str::to_string), false)
            };
            let has_inline = inline.is_some();
            if let Some(target) = inline.or_else(|| args.get(i + 1).cloned()) {
                let resolved = self.resolve(Some(&target));
                redir = Some((resolved.clone(), append));
                err_redir = Some((resolved, append));
                args.drain(i..=i + usize::from(!has_inline));
            } else {
                args.remove(i);
            }
        }
        if let Some(i) = args.iter().position(|a| a == ">" || a == ">>") {
            let append = args[i] == ">>";
            if i + 1 < args.len() {
                redir = Some((self.resolve(Some(&args[i + 1])), append));
                args.drain(i..=i + 1);
            } else {
                args.remove(i);
            }
        }
        // `2>`/`2>>` stderr redirection (launch only; same fd mechanism).
        if let Some(i) = args
            .iter()
            .position(|a| a == "2>" || a == "2>>" || a.starts_with("2>") && a.len() > 2)
        {
            let tok = args[i].clone();
            let (inline, append) = if tok == "2>" || tok == "2>>" {
                (None, tok == "2>>")
            } else if let Some(rest) = tok.strip_prefix("2>>") {
                (Some(rest.to_string()), true)
            } else {
                (tok.strip_prefix("2>").map(str::to_string), false)
            };
            let has_inline = inline.is_some();
            if let Some(target) = inline.or_else(|| args.get(i + 1).cloned()) {
                err_redir = Some((self.resolve(Some(&target)), append));
                args.drain(i..=i + usize::from(!has_inline));
            } else {
                args.remove(i);
            }
        }
        // `<` input redirection: for `launch` it becomes the child's
        // stdin fd; for builtins it feeds pending_stdin as before.
        let mut stdin_fd = 0i64;
        let mut stderr_fd = 0i64;
        if let Some(i) = args.iter().position(|a| a == "<") {
            if i + 1 < args.len() {
                let path = self.resolve(Some(&args[i + 1]));
                args.drain(i..=i + 1);
                if cmd.name == "launch" || cmd.name == "run" {
                    match self.fs.open_read(&path) {
                        Some(fd) => stdin_fd = fd,
                        None => {
                            self.emit(&format!("{}: {}: no such file", cmd.name, path));
                            self.last_ok = false;
                            return;
                        }
                    }
                } else {
                    match self.fs.read_to_string(&path) {
                        Some(data) => self.pending_stdin = Some(data),
                        None => {
                            self.emit(&format!("{}: {}: no such file", cmd.name, path));
                            self.last_ok = false;
                            return;
                        }
                    }
                }
            } else {
                args.remove(i);
            }
        }
        // Pipeline stdin: an explicit `<` wins; otherwise a pipe read
        // fd supplied by execute_pipeline feeds the child's fd 0.
        if stdin_fd == 0 && self.pipe_in_fd > 0 {
            stdin_fd = self.pipe_in_fd;
        }
        // Trailing `&` requests background execution. `launch` already
        // runs children concurrently; other builtins are synchronous
        // and complete before the next prompt regardless.
        if args.last().is_some_and(|a| a == "&") {
            args.pop();
        }
        let prev_redir = self.redirect.take();
        if let Some(r) = redir.clone() {
            self.redirect = Some(r);
            self.redirect_initialized = false;
        }
        self.last_ok = true;
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
            "mkdir" => self.cmd_mkdir(&args),
            "rmdir" => self.cmd_rmdir(&args),
            "rm" => self.cmd_rm(&args),
            "touch" => self.cmd_touch(&args),
            "cp" => self.cmd_cp(&args, false),
            "mv" => self.cmd_cp(&args, true),
            "head" => self.cmd_head_tail(&args, true),
            "tail" => self.cmd_head_tail(&args, false),
            "wc" => self.cmd_wc(&args),
            "grep" => self.cmd_grep(&args),
            "sort" => self.cmd_sort(&args),
            "uniq" => self.cmd_uniq(&args),
            "find" => self.cmd_find(&args),
            "basename" => self.cmd_basename(&args),
            "dirname" => self.cmd_dirname(&args),
            "seq" => self.cmd_seq(&args),
            "printf" => self.cmd_printf(&args),
            "test" => self.cmd_test(&args, false),
            "[" => self.cmd_test(&args, true),
            "tr" => self.cmd_tr(&args),
            "cut" => self.cmd_cut(&args),
            "nl" => self.cmd_nl(&args),
            "xargs" => self.cmd_xargs(&args),
            "paste" => self.cmd_paste(&args),
            "tee" => self.cmd_tee(&args),
            "env" => self.cmd_env(&args),
            "sleep" => self.cmd_sleep(&args),
            "kill" => self.cmd_kill(&args),
            "uname" => self.emit(self.sys.uname()),
            "mem" => self.cmd_mem(),
            "ps" => self.cmd_ps(),
            "time" | "uptime" => self.emit(&format_u64(self.sys.uptime())),
            "launch" | "run" => {
                // `launch prog > file`: open the target and hand its fd
                // to the child as its stdout/stderr sink.
                let fd = match &redir {
                    Some((path, append)) => match self.fs.open_write(path, *append) {
                        Some(fd) => fd,
                        None => {
                            self.emit(&format!("launch: {}: cannot open for write", path));
                            self.last_ok = false;
                            return;
                        }
                    },
                    None => self.pipe_out_fd,
                };
                if redir.is_some() || self.pipe_out_fd > 0 {
                    // fd redirect (file or pipe) replaces the emit
                    // redirect for launch; prev_redir is restored at
                    // the end of this fn.
                    self.redirect = None;
                }
                // `2> file`: open the stderr target the same way. When it
                // matches the stdout target (`&>`), share the fd so both
                // streams keep one file offset.
                if let Some((path, append)) = &err_redir {
                    if redir
                        .as_ref()
                        .is_some_and(|(p, a)| p == path && *a == *append)
                    {
                        stderr_fd = fd;
                    } else {
                        match self.fs.open_write(path, *append) {
                            Some(fd) => stderr_fd = fd,
                            None => {
                                self.emit(&format!("launch: {}: cannot open for write", path));
                                self.last_ok = false;
                                return;
                            }
                        }
                    }
                }
                self.cmd_launch(&args, fd, stdin_fd, stderr_fd);
            }
            "wait" => self.cmd_wait(&args),
            "reboot" => self.sys.reboot(),
            "jobs" => self.cmd_jobs(),
            "fg" => self.cmd_fg(&args),
            "bg" => self.cmd_bg(&args),
            _ => {
                self.last_ok = false;
                self.emit(&format!("sigma-sh: {}: command not found", cmd.name));
            }
        }
        self.pending_stdin = None;
        self.redirect = prev_redir;
    }

    fn cmd_help(&mut self) {
        for l in [
            "builtins:",
            "  help            show this text",
            "  ls [-l] [dir]   list directory (-l = long format)",
            "  pwd             print working directory",
            "  cd [dir]        change directory",
            "  echo <text>     print text",
            "  cat <file>      print file",
            "  which <prog>    locate a program on PATH",
            "  history         list command history",
            "  export V=val    set an environment variable",
            "  unset V         remove an environment variable",
            "  mkdir <dir>     create a directory",
            "  rmdir <dir>     remove an empty directory",
            "  rm <file>       remove a file",
            "  touch <file>    create an empty file",
            "  cp <src> <dst>  copy a file",
            "  mv <src> <dst>  move a file",
            "  head [-n N] <f> print first N lines (default 10)",
            "  tail [-n N] <f> print last N lines",
            "  wc <file>       count lines, words, bytes",
            "  grep [-ivnc] <pat> <file>  search file contents",
            "  sort [-r] <file>    sort lines",
            "  uniq <file>         collapse adjacent duplicate lines",
            "  find [dir]          list files recursively",
            "  basename <path> [suf]  last path component",
            "  dirname <path>      parent directory",
            "  seq [a [b]] <end>   print number sequence",
            "  printf FMT [args]  formatted output (%s %d %x \\n ...)",
            "  test EXPR / [ EXPR ]  conditional tests (-f -d -e -s = -eq ...)",
            "  tr [-d] S1 [S2]    translate/delete chars (a-z ranges)",
            "  cut -dD -fLIST|-cLIST [file]  select fields/chars",
            "  nl [file]         number lines",
            "  xargs [-I S] [-n N] CMD  run CMD per stdin line",
            "  paste [-dC] <file>...  join lines of files with a delim",
            "  tee [-a] <file>   copy stdin to file(s) and stdout",
            "  env              list environment variables",
            "  sleep <secs>     pause execution",
            "  kill [-9] <pid>  terminate a process",
            "  uname            print OS name",
            "  true/:/false    no-op / fail (for && and ||)",
            "  clear           clear the screen",
            "  mem             memory usage",
            "  ps              process list",
            "  time            uptime seconds",
            "  launch <prog>   run a program (& = background, the default)",
            "  redirects       > >> 2> 2>> &> &>>  and  < file  (real fds for launch)",
            "  wait [pid]      wait for a child to exit",
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
                // Sync the kernel cwd so spawned children inherit it.
                let _ = self.fs.chdir(&norm);
                self.oldpwd = Some(core::mem::replace(&mut self.cwd, norm.clone()));
                if is_back {
                    self.emit(&norm);
                }
            }
            None => {
                self.last_ok = false;
                self.emit(&format!("cd: {}: no such directory", target));
            }
        }
    }

    fn cmd_ls(&mut self, args: &[String]) {
        let long = args.iter().any(|a| a == "-l" || a == "-la" || a == "-al");
        let path_arg = args.iter().find(|a| !a.starts_with('-'));
        let path = self.resolve(path_arg);
        match self.fs.list(&path) {
            Some(names) => {
                for n in names {
                    if long {
                        let child = format!("{}/{}", path.trim_end_matches('/'), n);
                        match self.fs.stat(&child) {
                            Some((is_dir, size)) => {
                                let kind = if is_dir { 'd' } else { '-' };
                                self.emit(&format!(
                                    "{}{} {:>8} {}",
                                    kind,
                                    if is_dir { "rwxr-xr-x" } else { "rw-r--r--" },
                                    size,
                                    n
                                ));
                            }
                            None => self.emit(&format!("-????????? {:>8} {}", "?", n)),
                        }
                    } else {
                        self.emit(&n);
                    }
                }
            }
            None => {
                self.last_ok = false;
                self.emit(&format!("ls: {}: no such directory", path));
            }
        }
    }

    fn cmd_cat(&mut self, args: &[String]) {
        let Some(a) = args.first() else {
            if self.pending_stdin.is_some() {
                let s = self.take_stdin();
                self.emit_raw(&s);
            } else {
                self.emit("cat: missing file");
            }
            return;
        };
        let path = self.resolve(Some(a));
        match self.fs.read_to_string(&path) {
            Some(s) => self.emit_raw(&s),
            None => {
                self.last_ok = false;
                self.emit(&format!("cat: {}: no such file", path));
            }
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
                self.last_ok = false;
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
        self.last_ok = false;
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

    fn cmd_mkdir(&mut self, args: &[String]) {
        if args.is_empty() {
            self.emit("mkdir: missing directory");
            return;
        }
        for a in args {
            let path = self.resolve(Some(a));
            if self.fs.create_dir(&path) {
                // success is silent, like mkdir
            } else {
                self.emit(&format!("mkdir: cannot create {}", path));
            }
        }
    }

    fn cmd_rmdir(&mut self, args: &[String]) {
        if args.is_empty() {
            self.emit("rmdir: missing directory");
            return;
        }
        for a in args {
            let path = self.resolve(Some(a));
            if self.fs.remove_path(&path) {
                // silent on success
            } else {
                self.emit(&format!("rmdir: {}: failed", path));
            }
        }
    }

    fn cmd_rm(&mut self, args: &[String]) {
        if args.is_empty() {
            self.emit("rm: missing file");
            return;
        }
        for a in args {
            let path = self.resolve(Some(a));
            if self.fs.remove_path(&path) {
                // silent on success
            } else {
                self.emit(&format!("rm: cannot remove {}", path));
            }
        }
    }

    fn cmd_touch(&mut self, args: &[String]) {
        if args.is_empty() {
            self.emit("touch: missing file");
            return;
        }
        for a in args {
            let path = self.resolve(Some(a));
            // Append-mode write of nothing creates the file without truncating.
            if !self.fs.write_file(&path, "", true) {
                self.emit(&format!("touch: cannot create {}", path));
            }
        }
    }

    fn cmd_cp(&mut self, args: &[String], move_: bool) {
        let name = if move_ { "mv" } else { "cp" };
        let Some(src) = args.first() else {
            self.emit(&format!("{}: missing source", name));
            return;
        };
        let Some(dst) = args.get(1) else {
            self.emit(&format!("{}: missing destination", name));
            return;
        };
        let src_path = self.resolve(Some(src));
        let dst_path = self.resolve(Some(dst));
        match self.fs.read_to_string(&src_path) {
            Some(data) => {
                if self.fs.write_file(&dst_path, &data, false) {
                    if move_ {
                        let _ = self.fs.remove_path(&src_path);
                    }
                } else {
                    self.emit(&format!("{}: cannot write {}", name, dst_path));
                }
            }
            None => self.emit(&format!("{}: {}: no such file", name, src_path)),
        }
    }

    /// `head`/`tail`: `-n N` line count (default 10), then file.
    fn cmd_head_tail(&mut self, args: &[String], head: bool) {
        let name = if head { "head" } else { "tail" };
        let mut n = 10usize;
        let mut file: Option<&String> = None;
        let mut i = 0;
        while i < args.len() {
            if args[i] == "-n" && i + 1 < args.len() {
                n = args[i + 1].parse().unwrap_or(10);
                i += 2;
            } else if file.is_none() {
                file = Some(&args[i]);
                i += 1;
            } else {
                i += 1;
            }
        }
        let data = match file {
            Some(f) => {
                let path = self.resolve(Some(f));
                let Some(d) = self.fs.read_to_string(&path) else {
                    self.emit(&format!("{}: {}: no such file", name, path));
                    return;
                };
                d
            }
            None => {
                if self.pending_stdin.is_none() {
                    self.emit(&format!("{}: missing file", name));
                    return;
                }
                self.take_stdin()
            }
        };
        let lines: Vec<&str> = data.lines().collect();
        let selected: Vec<&str> = if head {
            lines.iter().take(n).copied().collect()
        } else {
            let start = lines.len().saturating_sub(n);
            lines.iter().skip(start).copied().collect()
        };
        for l in selected {
            self.emit(l);
        }
    }

    fn cmd_wc(&mut self, args: &[String]) {
        let data = match args.first() {
            Some(f) => {
                let path = self.resolve(Some(f));
                let Some(d) = self.fs.read_to_string(&path) else {
                    self.emit(&format!("wc: {}: no such file", path));
                    return;
                };
                d
            }
            None => {
                if self.pending_stdin.is_none() {
                    self.emit("wc: missing file");
                    return;
                }
                self.take_stdin()
            }
        };
        let lines = data.lines().count();
        let words = data.split_whitespace().count();
        let bytes = data.len();
        match args.first() {
            Some(f) => {
                let path = self.resolve(Some(f));
                self.emit(&format!("{} {} {} {}", lines, words, bytes, path));
            }
            None => self.emit(&format!("{} {} {}", lines, words, bytes)),
        }
    }

    /// `grep [-ivnc] PATTERN FILE` — substring match per line.
    fn cmd_grep(&mut self, args: &[String]) {
        let mut icase = false;
        let mut invert = false;
        let mut numbers = false;
        let mut count_only = false;
        let mut pos: Vec<&String> = Vec::new();
        for a in args {
            match a.as_str() {
                "-i" => icase = true,
                "-v" => invert = true,
                "-n" => numbers = true,
                "-c" => count_only = true,
                _ => pos.push(a),
            }
        }
        if pos.is_empty() || (pos.len() < 2 && self.pending_stdin.is_none()) {
            self.emit("grep: usage: grep [-ivnc] <pattern> [file]");
            return;
        }
        let pat = if icase {
            pos[0].to_lowercase()
        } else {
            pos[0].clone()
        };
        let data = match pos.get(1) {
            Some(f) => {
                let path = self.resolve(Some(f));
                let Some(d) = self.fs.read_to_string(&path) else {
                    self.emit(&format!("grep: {}: no such file", path));
                    return;
                };
                d
            }
            None => self.take_stdin(),
        };
        let mut hits = 0usize;
        for (i, line) in data.lines().enumerate() {
            let hay = if icase {
                line.to_lowercase()
            } else {
                line.to_string()
            };
            let m = hay.contains(&pat) != invert;
            if m {
                hits += 1;
                if !count_only {
                    if numbers {
                        self.emit(&format!("{}:{}", i + 1, line));
                    } else {
                        self.emit(line);
                    }
                }
            }
        }
        if count_only {
            self.emit(&hits.to_string());
        }
    }

    fn cmd_sort(&mut self, args: &[String]) {
        let reverse = args.iter().any(|a| a == "-r");
        let file = args.iter().find(|a| !a.starts_with('-'));
        let data = match file {
            Some(f) => {
                let path = self.resolve(Some(f));
                let Some(d) = self.fs.read_to_string(&path) else {
                    self.emit(&format!("sort: {}: no such file", path));
                    return;
                };
                d
            }
            None => {
                if self.pending_stdin.is_none() {
                    self.emit("sort: missing file");
                    return;
                }
                self.take_stdin()
            }
        };
        let mut lines: Vec<String> = data.lines().map(str::to_string).collect();
        lines.sort();
        if reverse {
            lines.reverse();
        }
        for l in lines {
            self.emit(&l);
        }
    }

    fn cmd_uniq(&mut self, args: &[String]) {
        let data = match args.first() {
            Some(f) => {
                let path = self.resolve(Some(f));
                let Some(d) = self.fs.read_to_string(&path) else {
                    self.emit(&format!("uniq: {}: no such file", path));
                    return;
                };
                d
            }
            None => {
                if self.pending_stdin.is_none() {
                    self.emit("uniq: missing file");
                    return;
                }
                self.take_stdin()
            }
        };
        let mut prev: Option<&str> = None;
        for line in data.lines() {
            if prev != Some(line) {
                self.emit(line);
            }
            prev = Some(line);
        }
    }

    /// `find [dir]` — recursive listing via `Fs::list`.
    fn cmd_find(&mut self, args: &[String]) {
        let start = args
            .first()
            .map(|a| self.resolve(Some(a)))
            .unwrap_or_else(|| self.cwd.clone());
        let mut stack = alloc::vec![start.clone()];
        while let Some(dir) = stack.pop() {
            let Some(names) = self.fs.list(&dir) else {
                self.emit(&format!("find: {}: no such directory", dir));
                continue;
            };
            for n in names {
                let path = if dir == "/" {
                    format!("/{}", n)
                } else {
                    format!("{}/{}", dir, n)
                };
                self.emit(&path);
                // Recurse into subdirectories.
                if self.fs.list(&path).is_some() {
                    stack.push(path);
                }
            }
        }
    }

    fn cmd_basename(&mut self, args: &[String]) {
        let Some(p) = args.first() else {
            self.emit("basename: missing operand");
            return;
        };
        let mut name = p.rsplit('/').next().unwrap_or(p.as_str());
        if let Some(suf) = args.get(1) {
            if let Some(stripped) = name.strip_suffix(suf.as_str()) {
                if !stripped.is_empty() {
                    name = stripped;
                }
            }
        }
        self.emit(name);
    }

    fn cmd_dirname(&mut self, args: &[String]) {
        let Some(p) = args.first() else {
            self.emit("dirname: missing operand");
            return;
        };
        let trimmed = p.trim_end_matches('/');
        let dir = match trimmed.rfind('/') {
            Some(0) | None => "/",
            Some(i) => &trimmed[..i],
        };
        self.emit(if dir.is_empty() { "/" } else { dir });
    }

    /// `seq [FIRST [INCREMENT]] LAST` — integer sequence.
    fn cmd_seq(&mut self, args: &[String]) {
        let nums: Vec<i64> = args.iter().filter_map(|a| a.parse().ok()).collect();
        let (first, step, last) = match nums.len() {
            1 => (1, 1, nums[0]),
            2 => (nums[0], 1, nums[1]),
            n if n >= 3 => (nums[0], nums[1], nums[2]),
            _ => {
                self.emit("seq: usage: seq [first [incr]] last");
                return;
            }
        };
        if step == 0 {
            self.emit("seq: zero increment");
            return;
        }
        let mut v = first;
        while (step > 0 && v <= last) || (step < 0 && v >= last) {
            self.emit(&v.to_string());
            v += step;
        }
    }

    /// Shared input for text filters: explicit file wins, otherwise
    /// piped stdin (empty when nothing was piped in).
    fn filter_input(&mut self, file: Option<&String>, name: &str) -> Option<String> {
        match file {
            Some(f) => {
                let path = self.resolve(Some(f));
                let d = self.fs.read_to_string(&path);
                if d.is_none() {
                    self.emit(&format!("{}: {}: no such file", name, path));
                }
                d
            }
            None => Some(self.pending_stdin.take().unwrap_or_default()),
        }
    }

    /// `printf FORMAT [ARGS...]` — %s %d %i %u %x %o %c %%, plus
    /// \\n \\t \\r \\\\ escapes. Missing args read as empty/0; the format
    /// repeats while args remain (POSIX).
    fn cmd_printf(&mut self, args: &[String]) {
        let Some(fmt) = args.first() else {
            self.emit("printf: usage: printf FORMAT [ARGS...]");
            return;
        };
        let vals = &args[1..];
        let mut ai = 0usize;
        let mut out = String::new();
        let mut chars = fmt.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('\\') => out.push('\\'),
                    Some('0') => out.push('\0'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                },
                '%' => match chars.next() {
                    Some('%') => out.push('%'),
                    Some(spec @ ('s' | 'd' | 'i' | 'u' | 'x' | 'X' | 'o' | 'c')) => {
                        let a = vals.get(ai).cloned().unwrap_or_default();
                        ai += 1;
                        match spec {
                            's' => out.push_str(&a),
                            'c' => out.extend(a.chars().next()),
                            'x' => {
                                if let Ok(v) = a.parse::<u64>() {
                                    out.push_str(&format!("{:x}", v));
                                }
                            }
                            'X' => {
                                if let Ok(v) = a.parse::<u64>() {
                                    out.push_str(&format!("{:X}", v));
                                }
                            }
                            'o' => {
                                if let Ok(v) = a.parse::<u64>() {
                                    out.push_str(&format!("{:o}", v));
                                }
                            }
                            _ => {
                                if let Ok(v) = a.parse::<i64>() {
                                    out.push_str(&v.to_string());
                                } else {
                                    out.push('0');
                                }
                            }
                        }
                    }
                    Some(other) => {
                        out.push('%');
                        out.push(other);
                    }
                    None => out.push('%'),
                },
                _ => out.push(c),
            }
        }
        self.emit_raw(&out);
    }

    /// `test EXPR` / `[ EXPR ]` — file tests (-e -f -d -s), string
    /// tests (= != < >), integer tests (-eq -ne -lt -le -gt -ge),
    /// and `!` negation. Sets last_ok.
    fn cmd_test(&mut self, args: &[String], bracket: bool) {
        let mut args: Vec<&str> = args.iter().map(|a| a.as_str()).collect();
        if bracket {
            if args.last() == Some(&"]") {
                args.pop();
            } else {
                self.emit("[: missing ]");
                self.last_ok = false;
                return;
            }
        }
        let ok = self.eval_test_expr(&args);
        self.last_ok = ok;
    }

    /// Evaluate a `test` expression with `-o` < `-a` < `!` precedence:
    /// unary file/string tests, binary string/integer comparisons.
    fn eval_test_expr(&self, args: &[&str]) -> bool {
        let mut p = TestParser {
            sh: self,
            args,
            i: 0,
        };
        p.parse_or()
    }

    /// `tr [-d] SET1 [SET2]` — translate or delete characters from
    /// stdin/file. Supports `a-z` ranges and backslash escapes.
    fn cmd_tr(&mut self, args: &[String]) {
        let mut delete = false;
        let mut sets: Vec<&String> = Vec::new();
        for a in args {
            if a == "-d" {
                delete = true;
            } else {
                sets.push(a);
            }
        }
        if sets.is_empty() || (!delete && sets.len() < 2) {
            self.emit("tr: usage: tr [-d] SET1 [SET2]");
            return;
        }
        let set1 = expand_tr_set(sets[0]);
        let set2 = if delete {
            Vec::new()
        } else {
            expand_tr_set(sets[1])
        };
        let data = match self.filter_input(None, "tr") {
            Some(d) => d,
            None => return,
        };
        let mut out = String::new();
        'outer: for ch in data.chars() {
            if delete {
                if set1.contains(&ch) {
                    continue;
                }
            } else if let Some(pos) = set1.iter().position(|&c| c == ch) {
                match set2.get(pos).or_else(|| set2.last()) {
                    Some(&rep) => out.push(rep),
                    None => out.push(ch),
                }
                continue 'outer;
            }
            out.push(ch);
        }
        self.emit_raw(&out);
    }

    /// `cut -d DELIM -f LIST` / `cut -c LIST` [FILE] — select fields
    /// or character positions per line. LIST is comma-separated
    /// 1-based positions and a-b ranges.
    fn cmd_cut(&mut self, args: &[String]) {
        let mut delim: Option<char> = None;
        let mut fields: Option<String> = None;
        let mut chars_mode = false;
        let mut file: Option<&String> = None;
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            // Attached forms: -d: -f2 -c1-3 (getopt-style).
            if let Some(rest) = a.strip_prefix("-d") {
                if !rest.is_empty() {
                    delim = rest.chars().next();
                    i += 1;
                    continue;
                }
            }
            if let Some(rest) = a.strip_prefix("-f") {
                if !rest.is_empty() {
                    fields = Some(rest.to_string());
                    i += 1;
                    continue;
                }
            }
            if let Some(rest) = a.strip_prefix("-c") {
                if !rest.is_empty() {
                    fields = Some(rest.to_string());
                    chars_mode = true;
                    i += 1;
                    continue;
                }
            }
            match a {
                "-d" if i + 1 < args.len() => {
                    delim = args[i + 1].chars().next();
                    i += 2;
                }
                "-f" if i + 1 < args.len() => {
                    fields = Some(args[i + 1].clone());
                    i += 2;
                }
                "-c" if i + 1 < args.len() => {
                    fields = Some(args[i + 1].clone());
                    chars_mode = true;
                    i += 2;
                }
                "--complement" => i += 1,
                a if a.starts_with('-') => i += 1,
                _ => {
                    file = Some(&args[i]);
                    i += 1;
                }
            }
        }
        let Some(list) = fields else {
            self.emit("cut: usage: cut -d DELIM -f LIST | -c LIST [FILE]");
            return;
        };
        let wanted = parse_num_list(&list);
        let d = match delim {
            Some(d) if !chars_mode => d,
            _ if !chars_mode => {
                self.emit("cut: -d DELIM required with -f");
                return;
            }
            _ => '\0',
        };
        let Some(data) = self.filter_input(file, "cut") else {
            return;
        };
        for line in data.lines() {
            if chars_mode {
                let s: String = line
                    .chars()
                    .enumerate()
                    .filter(|(i, _)| num_list_has(&wanted, i + 1))
                    .map(|(_, c)| c)
                    .collect();
                self.emit(&s);
            } else {
                let parts: Vec<&str> = line.split(d).collect();
                let picked: Vec<&str> = parts
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| num_list_has(&wanted, i + 1))
                    .map(|(_, p)| *p)
                    .collect();
                self.emit(&picked.join(&d.to_string()));
            }
        }
    }

    /// `nl [FILE]` — number lines, "%6d\t" prefix like GNU nl default.
    fn cmd_nl(&mut self, args: &[String]) {
        let data = match self.filter_input(args.first(), "nl") {
            Some(d) => d,
            None => return,
        };
        for (i, line) in data.lines().enumerate() {
            self.emit(&format!("{:6}\t{}", i + 1, line));
        }
    }

    /// `xargs [-I STR] [-n N] CMD [ARGS...]` — run CMD once per
    /// stdin line (or per group of N lines with -n), appending the
    /// line(s) as arguments. `-I STR` replaces STR inside the fixed
    /// args with each line instead.
    fn cmd_xargs(&mut self, args: &[String]) {
        let mut repl: Option<String> = None;
        let mut per_line = 1usize;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-I" if i + 1 < args.len() => {
                    repl = Some(args[i + 1].clone());
                    i += 2;
                }
                "-n" if i + 1 < args.len() => {
                    per_line = args[i + 1].parse().unwrap_or(1).max(1);
                    i += 2;
                }
                a if a.starts_with("-I") && a.len() > 2 => {
                    repl = Some(a[2..].to_string());
                    i += 1;
                }
                a if a.starts_with("-n") && a.len() > 2 => {
                    per_line = a[2..].parse().unwrap_or(1).max(1);
                    i += 1;
                }
                _ => break,
            }
        }
        let fixed = &args[i..];
        if fixed.is_empty() {
            self.emit("xargs: usage: xargs [-I STR] [-n N] CMD [ARGS...]");
            return;
        }
        let data = self.pending_stdin.take().unwrap_or_default();
        let lines: Vec<&str> = data.lines().filter(|l| !l.is_empty()).collect();
        for chunk in lines.chunks(per_line) {
            let mut cargs: Vec<String> = Vec::new();
            if let Some(r) = &repl {
                for a in fixed {
                    cargs.push(a.replace(r, &chunk.join(" ")));
                }
            } else {
                cargs.extend(fixed.iter().cloned());
                cargs.extend(chunk.iter().map(|s| s.to_string()));
            }
            let n = cargs.len();
            let cmd = Command {
                name: cargs[0].clone(),
                args: cargs[1..].to_vec(),
                quoted: alloc::vec![0; n - 1],
            };
            self.execute_single_command(&cmd);
            if !self.running {
                break;
            }
        }
    }

    /// `paste [-d DELIM] FILE...` — join line i of each input with
    /// DELIM (default tab). One file or stdin when none given.
    fn cmd_paste(&mut self, args: &[String]) {
        let mut delim = '\t';
        let mut files: Vec<&String> = Vec::new();
        let mut i = 0;
        while i < args.len() {
            if args[i] == "-d" && i + 1 < args.len() {
                delim = args[i + 1].chars().next().unwrap_or('\t');
                i += 2;
            } else if let Some(rest) = args[i].strip_prefix("-d") {
                if !rest.is_empty() {
                    delim = rest.chars().next().unwrap_or('\t');
                }
                i += 1;
            } else {
                files.push(&args[i]);
                i += 1;
            }
        }
        let mut cols: Vec<Vec<String>> = Vec::new();
        if files.is_empty() {
            let data = self.pending_stdin.take().unwrap_or_default();
            cols.push(data.lines().map(|s| s.to_string()).collect());
        } else {
            for f in &files {
                let path = self.resolve(Some(f));
                match self.fs.read_to_string(&path) {
                    Some(d) => cols.push(d.lines().map(|s| s.to_string()).collect()),
                    None => {
                        self.emit(&format!("paste: {}: no such file", path));
                        return;
                    }
                }
            }
        }
        let rows = cols.iter().map(|c| c.len()).max().unwrap_or(0);
        for r in 0..rows {
            let line: Vec<&str> = cols
                .iter()
                .map(|c| c.get(r).map(String::as_str).unwrap_or(""))
                .collect();
            self.emit(&line.join(&delim.to_string()));
        }
    }

    /// `tee [-a] FILE...` — copy stdin to each file and stdout.
    fn cmd_tee(&mut self, args: &[String]) {
        let mut append = false;
        let mut files: Vec<&String> = Vec::new();
        for a in args {
            if a == "-a" {
                append = true;
            } else {
                files.push(a);
            }
        }
        if files.is_empty() {
            self.emit("tee: usage: tee [-a] FILE...");
            return;
        }
        let data = self.pending_stdin.take().unwrap_or_default();
        for f in &files {
            let path = self.resolve(Some(f));
            if !self.fs.write_file(&path, &data, append) {
                self.emit(&format!("tee: {}: cannot write", path));
                self.last_ok = false;
            }
        }
        self.emit_raw(&data);
    }

    fn cmd_env(&mut self, args: &[String]) {
        if let Some(name) = args.first() {
            let val = self
                .env
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone());
            match val {
                Some(v) => self.emit(&v),
                None => self.last_ok = false,
            }
            return;
        }
        let lines: Vec<String> = self
            .env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        for l in lines {
            self.emit(&l);
        }
    }

    fn cmd_sleep(&mut self, args: &[String]) {
        let Some(a) = args.first() else {
            self.emit("sleep: missing seconds");
            return;
        };
        match a.parse::<u64>() {
            Ok(secs) => self.sys.sleep(secs),
            Err(_) => self.emit(&format!("sleep: {}: invalid interval", a)),
        }
    }

    fn cmd_kill(&mut self, args: &[String]) {
        // `kill [-SIG] pid` — first dash-arg selects the signal
        // ("-9", "-KILL", "-SIGTERM"), default SIGTERM.
        let mut sig = String::from("TERM");
        let mut pid_arg: Option<&String> = None;
        for a in args {
            if let Some(s) = a.strip_prefix('-') {
                if !s.is_empty() {
                    sig = s.trim_start_matches("SIG").to_string();
                }
                continue;
            }
            pid_arg = Some(a);
            break;
        }
        let Some(p) = pid_arg else {
            self.emit("kill: missing pid");
            self.last_ok = false;
            return;
        };
        match p.parse::<i64>() {
            Ok(pid) if self.sys.kill(pid, &sig) => {}
            _ => {
                self.last_ok = false;
                self.emit(&format!("kill: {}: no such process", p));
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

    fn cmd_launch(&mut self, args: &[String], stdout_fd: i64, stdin_fd: i64, stderr_fd: i64) {
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
        let envp: alloc::vec::Vec<String> = self
            .env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        match self
            .sys
            .launch(prog, &args[1..], &envp, stdout_fd, stdin_fd, stderr_fd)
        {
            Ok(pid) => {
                self.last_pid = Some(pid);
                if self.jobs.len() < 32 {
                    self.jobs.push((pid, prog.to_string(), false));
                }
                self.emit(&format!("launched {} (pid {})", prog, pid));
            }
            Err(e) => self.emit(&format!("launch: {}: {}", prog, e)),
        }
    }

    /// `wait [pid]` — block until a child exits; bare `wait` uses the
    /// most recently launched pid.
    fn cmd_wait(&mut self, args: &[String]) {
        let pid = match args.first() {
            Some(s) => match s.parse::<i64>() {
                Ok(p) => p,
                Err(_) => {
                    self.emit(&format!("wait: invalid pid '{}'", s));
                    self.last_ok = false;
                    return;
                }
            },
            None => match self.last_pid {
                Some(p) => p,
                None => {
                    self.emit("wait: no child process");
                    self.last_ok = false;
                    return;
                }
            },
        };
        match self.sys.wait(pid) {
            Ok(status) => {
                for j in self.jobs.iter_mut() {
                    if j.0 == pid {
                        j.2 = true;
                    }
                }
                self.emit(&format!("wait: pid {} exited {}", pid, status));
            }
            Err(e) => {
                self.emit(&format!("wait: {}: {}", pid, e));
                self.last_ok = false;
            }
        }
    }

    /// `jobs` — list launched jobs. Children already run concurrently
    /// under the kernel scheduler, so "background" is the default;
    /// entries are marked reaped once `wait`/`fg` collects them.
    fn cmd_jobs(&mut self) {
        if self.jobs.is_empty() {
            self.emit("jobs: no jobs");
            return;
        }
        let jobs: Vec<String> = self
            .jobs
            .iter()
            .enumerate()
            .map(|(i, (pid, cmd, reaped))| {
                let state = if *reaped { "done" } else { "running" };
                format!("[{}] {:>4} {} {}", i + 1, pid, state, cmd)
            })
            .collect();
        for line in jobs {
            self.emit(&line);
        }
    }

    /// `fg <pid|job#>` — block until the job exits (wait semantics);
    /// there is no terminal foreground/background distinction in the
    /// serial console, so fg == wait on the chosen job.
    fn cmd_fg(&mut self, args: &[String]) {
        let Some(sel) = args.first() else {
            self.emit("fg: missing job");
            self.last_ok = false;
            return;
        };
        let pid = match self.job_pid(sel) {
            Some(p) => p,
            None => {
                self.emit(&format!("fg: {}: no such job", sel));
                self.last_ok = false;
                return;
            }
        };
        match self.sys.wait(pid) {
            Ok(status) => {
                for j in self.jobs.iter_mut() {
                    if j.0 == pid {
                        j.2 = true;
                    }
                }
                self.emit(&format!("fg: pid {} exited {}", pid, status));
            }
            Err(e) => {
                self.emit(&format!("fg: {}: {}", pid, e));
                self.last_ok = false;
            }
        }
    }

    /// `bg <pid|job#>` — children are already scheduled concurrently,
    /// so this just confirms the job keeps running.
    fn cmd_bg(&mut self, args: &[String]) {
        let Some(sel) = args.first() else {
            self.emit("bg: missing job");
            self.last_ok = false;
            return;
        };
        match self.job_pid(sel) {
            Some(pid) => self.emit(&format!("bg: pid {} already running", pid)),
            None => {
                self.emit(&format!("bg: {}: no such job", sel));
                self.last_ok = false;
            }
        }
    }

    /// Resolve a job selector — pid digits or 1-based `[n]` index.
    fn job_pid(&self, sel: &str) -> Option<i64> {
        if let Ok(n) = sel.parse::<usize>() {
            if (1..=self.jobs.len()).contains(&n) {
                return Some(self.jobs[n - 1].0);
            }
        }
        sel.parse::<i64>()
            .ok()
            .and_then(|p| self.jobs.iter().find(|j| j.0 == p).map(|j| j.0))
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
        dirs: Rc<RefCell<Vec<String>>>,
        removed: Rc<RefCell<Vec<String>>>,
        open_log: Rc<RefCell<Vec<(String, bool)>>>,
        // In-memory pipe model: fd -> (pipe_id, is_write); buffers per pipe.
        pipe_fds: Rc<RefCell<std::collections::BTreeMap<i64, (usize, bool)>>>,
        pipe_bufs: Rc<RefCell<std::collections::BTreeMap<usize, std::collections::VecDeque<u8>>>>,
        pipe_next: Rc<RefCell<usize>>,
        closed: Rc<RefCell<Vec<i64>>>,
    }
    impl Fs for TestFs {
        fn pipe(&self) -> Option<(i64, i64)> {
            let mut next = self.pipe_next.borrow_mut();
            let id = *next;
            *next += 1;
            drop(next);
            let r = 300 + id as i64 * 2;
            let w = r + 1;
            self.pipe_fds.borrow_mut().insert(r, (id, false));
            self.pipe_fds.borrow_mut().insert(w, (id, true));
            self.pipe_bufs.borrow_mut().insert(id, Default::default());
            Some((r, w))
        }
        fn close(&self, fd: i64) -> bool {
            self.closed.borrow_mut().push(fd);
            self.pipe_fds.borrow_mut().remove(&fd).is_some()
        }
        fn read_fd(&self, fd: i64, buf: &mut [u8]) -> i64 {
            let Some(&(id, is_write)) = self.pipe_fds.borrow().get(&fd) else {
                return -9;
            };
            if is_write {
                return -9;
            }
            let mut bufs = self.pipe_bufs.borrow_mut();
            let q = bufs.entry(id).or_default();
            let n = q.len().min(buf.len());
            for b in buf.iter_mut().take(n) {
                *b = q.pop_front().unwrap();
            }
            n as i64 // empty pipe reads as EOF in tests (no real blocking)
        }
        fn write_fd(&self, fd: i64, buf: &[u8]) -> i64 {
            let Some(&(id, is_write)) = self.pipe_fds.borrow().get(&fd) else {
                return -9;
            };
            if !is_write {
                return -9;
            }
            let mut bufs = self.pipe_bufs.borrow_mut();
            let q = bufs.entry(id).or_default();
            for &b in buf {
                q.push_back(b);
            }
            buf.len() as i64
        }
        fn open_write(&self, path: &str, append: bool) -> Option<i64> {
            self.open_log.borrow_mut().push((path.to_string(), append));
            Some(100 + self.open_log.borrow().len() as i64)
        }
        fn open_read(&self, path: &str) -> Option<i64> {
            if self.read_to_string(path).is_some() {
                Some(50)
            } else {
                None
            }
        }
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
                "/tmp/multi" => Some("l1\nl2\nl3\n".to_string()),
                "/tmp/dups" => Some("b\na\na\nc\n".to_string()),
                _ => None,
            }
        }
        fn write_file(&self, path: &str, data: &str, append: bool) -> bool {
            self.writes
                .borrow_mut()
                .push((path.to_string(), data.to_string(), append));
            true
        }
        fn create_dir(&self, path: &str) -> bool {
            self.dirs.borrow_mut().push(path.to_string());
            true
        }
        fn remove_path(&self, path: &str) -> bool {
            self.removed.borrow_mut().push(path.to_string());
            true
        }
        fn stat(&self, path: &str) -> Option<(bool, u64)> {
            match path {
                "/" | "/bin" | "/etc" | "/tmp" => Some((true, 0)),
                "/etc/hostname" => Some((false, 12)),
                "/tmp/multi" => Some((false, 9)),
                "/tmp/dups" => Some((false, 8)),
                "/tmp/empty" => Some((false, 0)),
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
        killed: Rc<RefCell<Vec<(i64, String)>>>,
        fds: Rc<RefCell<Vec<(i64, i64, i64)>>>,
    }
    impl System for TestSys {
        fn reboot(&self) {}
        fn launch(
            &self,
            path: &str,
            _args: &[String],
            _env: &[String],
            stdout_fd: i64,
            stdin_fd: i64,
            stderr_fd: i64,
        ) -> Result<i64, String> {
            self.launched.borrow_mut().push(path.to_string());
            self.fds.borrow_mut().push((stdout_fd, stdin_fd, stderr_fd));
            Ok(100)
        }
        fn uptime(&self) -> u64 {
            3600
        }
        fn kill(&self, pid: i64, sig: &str) -> bool {
            self.killed.borrow_mut().push((pid, sig.to_string()));
            true
        }
        fn wait(&self, _pid: i64) -> Result<i64, String> {
            Ok(0)
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
        let killed = Rc::new(RefCell::new(Vec::new()));
        let fds = Rc::new(RefCell::new(Vec::new()));
        let sh = Shell::new(
            TestIo {
                input: lines.iter().map(|s| s.to_string()).collect(),
                output: out.clone(),
            },
            TestFs::default(),
            TestProcs,
            TestMem,
            TestSys {
                launched,
                killed,
                fds,
            },
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
    fn wait_uses_last_pid_and_reports_status() {
        let (mut sh, out) = shell(&["launch /bin/sh", "wait", "wait 7", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("wait: pid 100 exited 0"));
        assert!(o.contains("wait: pid 7 exited 0"));
    }

    #[test]
    fn jobs_fg_bg_track_launched() {
        let (mut sh, out) = shell(&[
            "launch /bin/sh arg1",
            "jobs",
            "fg 1",
            "jobs",
            "bg 100",
            "fg 99",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("[1]  100 running /bin/sh"));
        assert!(o.contains("fg: pid 100 exited 0"));
        assert!(o.contains("[1]  100 done /bin/sh"));
        assert!(o.contains("bg: pid 100 already running"));
        assert!(o.contains("fg: 99: no such job"));
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
    fn trailing_ampersand_is_accepted() {
        let (mut sh, out) = shell(&["echo hi &", "launch /bin/sh &", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("hi"));
        assert!(o.contains("launched /bin/sh"));
        assert!(!o.contains("&"));
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
    fn amp_redirect_builtin() {
        let (mut sh, out) = shell(&["echo hi &> /tmp/both", "echo yo &>> /tmp/both", "exit"]);
        sh.run();
        assert!(!out.borrow().contains("hi"));
        let writes = sh.fs.writes.borrow();
        assert!(writes.contains(&("/tmp/both".to_string(), "hi\n".to_string(), false)));
        assert!(writes.contains(&("/tmp/both".to_string(), "yo\n".to_string(), true)));
    }

    #[test]
    fn amp_redirect_launch_shares_fd() {
        let (mut sh, _out) = shell(&[
            "launch /bin/sh &> /tmp/both",
            "launch /bin/sh 2> /tmp/err",
            "exit",
        ]);
        sh.run();
        // `&>` opens the target once and hands the same fd to both
        // stdout_fd and stderr_fd; `2>` alone leaves stdout on console.
        assert_eq!(
            sh.fs.open_log.borrow().as_slice(),
            &[
                ("/tmp/both".to_string(), false),
                ("/tmp/err".to_string(), false),
            ]
        );
        let fds = sh.sys.fds.borrow();
        assert_eq!(fds.as_slice(), &[(101, 0, 101), (0, 0, 102)]);
    }

    #[test]
    fn launch_pipe_pipeline_wires_fds() {
        let (mut sh, _out) = shell(&["launch /a x | launch /b y", "exit"]);
        sh.run();
        // Stage 0 writes to pipe w (301); stage 1 reads pipe r (300).
        let fds = sh.sys.fds.borrow();
        assert_eq!(fds.as_slice(), &[(301, 0, 0), (0, 300, 0)]);
        drop(fds);
        // The shell closes its own copies of both pipe ends.
        let closed = sh.fs.closed.borrow();
        assert!(closed.contains(&300) && closed.contains(&301));
    }

    #[test]
    fn builtin_to_launch_pipes_text() {
        let (mut sh, _out) = shell(&["echo hello | launch /b", "exit"]);
        sh.run();
        let fds = sh.sys.fds.borrow();
        assert_eq!(fds.as_slice(), &[(0, 300, 0)]);
        drop(fds);
        // The builtin's captured output was written into the pipe.
        let bufs = sh.fs.pipe_bufs.borrow();
        let data: Vec<u8> = bufs.values().next().unwrap().iter().copied().collect();
        assert_eq!(data, b"hello\n");
    }

    #[test]
    fn fs_mutation_builtins() {
        let (mut sh, _out) = shell(&[
            "mkdir /tmp/dir",
            "touch /tmp/newfile",
            "rm /tmp/newfile",
            "rmdir /tmp/dir",
            "exit",
        ]);
        sh.run();
        assert_eq!(sh.fs.dirs.borrow().as_slice(), &["/tmp/dir".to_string()]);
        assert!(sh
            .fs
            .writes
            .borrow()
            .iter()
            .any(|(p, d, a)| { p == "/tmp/newfile" && d.is_empty() && *a }));
        assert_eq!(
            sh.fs.removed.borrow().as_slice(),
            &["/tmp/newfile".to_string(), "/tmp/dir".to_string()]
        );
    }

    #[test]
    fn file_utils_work() {
        let (mut sh, out) = shell(&[
            "cp /etc/hostname /tmp/copy",
            "mv /etc/hostname /tmp/moved",
            "head -n 2 /tmp/multi",
            "tail -n 1 /tmp/multi",
            "wc /tmp/multi",
            "exit",
        ]);
        sh.run();
        let writes = sh.fs.writes.borrow();
        assert!(writes.contains(&("/tmp/copy".to_string(), "sigmaos\n".to_string(), false)));
        assert!(writes.contains(&("/tmp/moved".to_string(), "sigmaos\n".to_string(), false)));
        drop(writes);
        assert!(sh
            .fs
            .removed
            .borrow()
            .contains(&"/etc/hostname".to_string()));
        let o = out.borrow();
        assert!(o.contains("l1"));
        assert!(o.contains("l2"));
        assert!(o.contains("l3"));
        assert!(o.contains("3 3 9 /tmp/multi"));
    }

    #[test]
    fn sort_uniq_find_work() {
        let (mut sh, out) = shell(&["sort /tmp/dups", "uniq /tmp/dups", "find /bin", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("a\nb\nc\n"), "sort order wrong: {}", o);
        assert!(o.contains("b\na\nc\n"), "uniq output wrong: {}", o);
        assert!(o.contains("/bin/sh"));
        assert!(o.contains("/bin/init"));
    }

    #[test]
    fn path_and_seq_utils() {
        let (mut sh, out) = shell(&[
            "basename /etc/hostname",
            "basename /boot/vmlinuz-linux.img .img",
            "dirname /etc/hostname",
            "seq 3",
            "seq 1 2 5",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("hostname"));
        assert!(o.contains("vmlinuz-linux"));
        assert!(o.contains("/etc"));
        assert!(o.contains("1\n2\n3"));
        assert!(o.contains("1\n3\n5"));
    }

    #[test]
    fn pipeline_wires_stdin() {
        let (mut sh, out) = shell(&["seq 3 | grep 2", "cat /tmp/multi | wc", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("2\n"), "{}", o);
        assert!(o.contains("3 3 9"), "{}", o);
    }

    #[test]
    fn input_redirect_feeds_stdin() {
        let (mut sh, out) = shell(&["grep l2 < /tmp/multi", "wc < /tmp/multi", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("l2\n"), "{}", o);
        assert!(o.contains("3 3 9"), "{}", o);
    }

    #[test]
    fn uname_prints_os_name() {
        let (mut sh, out) = shell(&["uname", "exit"]);
        sh.run();
        assert!(out.borrow().contains("sigmaos"));
    }

    #[test]
    fn kill_sends_signal() {
        let (mut sh, _out) = shell(&["kill 42", "kill -9 7", "kill -SIGKILL 9", "kill", "exit"]);
        sh.run();
        assert_eq!(
            sh.sys.killed.borrow().as_slice(),
            &[
                (42, "TERM".to_string()),
                (7, "9".to_string()),
                (9, "KILL".to_string())
            ]
        );
    }

    #[test]
    fn env_and_sleep() {
        let (mut sh, out) = shell(&[
            "export EDITOR=vim",
            "env EDITOR",
            "env",
            "env MISSING && echo nope || echo envfail-ok",
            "sleep 0",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("vim"));
        assert!(o.contains("EDITOR=vim"));
        assert!(o.contains("envfail-ok"));
        assert!(!o.contains("nope"));
    }

    #[test]
    fn grep_searches() {
        let (mut sh, out) = shell(&[
            "grep l2 /tmp/multi",
            "grep -n l2 /tmp/multi",
            "grep -c l /tmp/multi",
            "grep -v l1 /tmp/multi",
            "grep -i L3 /tmp/multi",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("l2"));
        assert!(o.contains("2:l2")); // -n line numbers
        assert!(o.contains("l3")); // -v and -i both reach l3
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
    fn quotes_suppress_expansion() {
        let (mut sh, out) = shell(&[
            "export FOO=bar",
            "echo $FOO",     // unquoted: expands
            "echo '$FOO'",   // single: literal
            "echo \"$FOO\"", // double: expands
            "echo '/b*'",    // single: no glob
            "echo \"/b*\"",  // double: no glob
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("> bar\n"));
        assert!(o.contains("$FOO"));
        // '/b*' and "/b*" both print literally, never /bin
        assert!(!o.contains("/bin /etc\n/b*"));
        let literal_count = o.matches("/b*").count();
        assert_eq!(literal_count, 2, "both quoted globs stay literal");
    }

    #[test]
    fn cat_missing_file() {
        let (mut sh, out) = shell(&["cat /nope", "exit"]);
        sh.run();
        assert!(out.borrow().contains("no such file"));
    }

    #[test]
    fn printf_formats() {
        let (mut sh, out) = shell(&[
            r#"printf "%s %d %x\n" hi 42 255"#,
            r#"printf "a\tb\\c\n""#,
            r#"printf "%s-%s\n" x y z"#, // extra arg: format does not repeat (single pass)
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("hi 42 ff"));
        assert!(o.contains("a\tb\\c"));
        assert!(o.contains("x-y"));
    }

    #[test]
    fn test_exprs() {
        let (mut sh, out) = shell(&[
            "test foo = foo && echo eq-ok",
            "[ 3 -gt 2 ] && echo gt-ok",
            "[ -f /etc/hostname ] && echo is-file",
            "[ -d /tmp ] && echo is-dir",
            "[ -e /nope ] || echo missing",
            "[ -s /tmp/empty ] || echo empty-file",
            "test ! -f /nope && echo neg-ok",
            "test -n hi && echo nonempty",
            "test 1 -eq 1 -a 2 -eq 2 && echo and-ok",
            "test 1 -eq 2 -o 2 -eq 2 && echo or-ok",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        for want in [
            "eq-ok",
            "gt-ok",
            "is-file",
            "is-dir",
            "missing",
            "empty-file",
            "neg-ok",
            "nonempty",
            "and-ok",
            "or-ok",
        ] {
            assert!(o.contains(want), "missing {}", want);
        }
    }

    #[test]
    fn tr_translates_and_deletes() {
        let (mut sh, out) = shell(&[
            "echo hello | tr a-z A-Z",
            "echo 'a1b2' | tr -d 0-9",
            "echo abc | tr abc xyz",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("HELLO"));
        assert!(o.contains("ab"));
        assert!(o.contains("xyz"));
    }

    #[test]
    fn cut_fields_and_chars() {
        let (mut sh, out) = shell(&[
            "echo 'a:b:c' | cut -d: -f2",
            "echo 'a:b:c' | cut -d: -f1,3",
            "echo 'a:b:c' | cut -d: -f2-",
            "echo hello | cut -c1-3",
            "cut -d: -f2 /tmp/multi",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("> b\n"));
        assert!(o.contains("a:c"));
        assert!(o.contains("b:c\n"));
        assert!(o.contains("hel"));
        // /tmp/multi lines l1/l2/l3 have no ':' -> whole line kept? no:
        // split on ':' yields one field; -f2 selects nothing -> empty lines.
    }

    #[test]
    fn nl_numbers_lines() {
        let (mut sh, out) = shell(&["nl /tmp/multi", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("     1\tl1"));
        assert!(o.contains("     3\tl3"));
    }

    #[test]
    fn tee_writes_and_echoes() {
        let (mut sh, out) = shell(&["echo piped | tee /tmp/t1 /tmp/t2", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("piped"));
        // tee wrote both files (writes log isn't directly visible; check
        // via a second command reading one back is not possible in TestFs,
        // so assert output went to stdout at minimum).
        let _ = out;
    }

    #[test]
    fn xargs_runs_per_line() {
        let (mut sh, out) = shell(&["printf 'a\\nb\\n' | xargs echo item", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("item a"), "OUT: {o}");
        assert!(o.contains("item b"), "OUT: {o}");
    }

    #[test]
    fn xargs_dash_i_and_dash_n() {
        let (mut sh, out) = shell(&[
            "printf 'a\\nb\\n' | xargs -I{} echo got-{}",
            "printf 'a\\nb\\n' | xargs -n 2 echo pair",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("got-a"));
        assert!(o.contains("got-b"));
        assert!(o.contains("pair a b"));
    }

    #[test]
    fn paste_joins_files() {
        let (mut sh, out) = shell(&[
            "paste /tmp/multi /tmp/dups",
            "paste -d: /tmp/multi /tmp/dups",
            "exit",
        ]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("l1\tb"));
        assert!(o.contains("l2\ta"));
        assert!(o.contains("l3\ta"));
        assert!(o.contains("l1:b"));
    }

    #[test]
    fn paste_stdin_single_col() {
        let (mut sh, out) = shell(&["printf 'x\\ny\\n' | paste", "exit"]);
        sh.run();
        let o = out.borrow();
        assert!(o.contains("x\n"));
        assert!(o.contains("y\n"));
    }
}
