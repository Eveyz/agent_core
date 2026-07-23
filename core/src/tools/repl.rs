//! Session REPL tool — long-lived Python / Node runners with retained bindings.
//!
//! Protocol (length-prefixed, no prompt scraping):
//! - Request:  `{byte_len}\n{code_bytes}`
//! - Response: `{ok|err}\n{byte_len}\n{output_bytes}`
//!
//! Zero new crates; requires host `python3`/`python` and/or `node`.

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex as ParkingMutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex as AsyncMutex;

use super::{Tool, ToolUpdateFn};
use crate::runtime::ProcessSupervisor;
use crate::types::{EventSender, ToolExecutionMode};

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_OUTPUT_CHARS: usize = 200_000;

/// Compact Python bootstrap: shared globals, length-prefixed I/O.
const PYTHON_BOOTSTRAP: &str = r#"
import sys, traceback
G = {"__name__": "__main__"}
def _send(ok, text):
    data = text.encode("utf-8", errors="replace")
    sys.stdout.buffer.write((b"ok\n" if ok else b"err\n"))
    sys.stdout.buffer.write(f"{len(data)}\n".encode())
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()
while True:
    header = sys.stdin.buffer.readline()
    if not header:
        break
    try:
        n = int(header.strip())
    except Exception:
        _send(False, "invalid frame header")
        continue
    code = sys.stdin.buffer.read(n).decode("utf-8", errors="replace")
    out_buf = []
    class _C:
        def write(self, s):
            out_buf.append(s)
        def flush(self):
            pass
    real_out, real_err = sys.stdout, sys.stderr
    sys.stdout = sys.stderr = _C()
    ok = True
    try:
        try:
            result = eval(code, G)
            if result is not None:
                print(repr(result))
        except SyntaxError:
            exec(code, G)
    except SystemExit:
        raise
    except BaseException:
        ok = False
        traceback.print_exc()
    finally:
        sys.stdout, sys.stderr = real_out, real_err
    _send(ok, "".join(out_buf))
"#;

/// Compact Node bootstrap: shared VM context, length-prefixed I/O.
const NODE_BOOTSTRAP: &str = r#"
const fs = require("fs");
const util = require("util");
const vm = require("vm");
const context = {
  console, require, process, Buffer, setTimeout, clearTimeout,
  setInterval, clearInterval, setImmediate, clearImmediate,
};
context.global = context;
context.globalThis = context;
vm.createContext(context);
function readExact(n) {
  const buf = Buffer.alloc(n);
  let off = 0;
  while (off < n) {
    const r = fs.readSync(0, buf, off, n - off);
    if (r === 0) return null;
    off += r;
  }
  return buf;
}
function readLine() {
  let s = "";
  const b = Buffer.alloc(1);
  for (;;) {
    const r = fs.readSync(0, b, 0, 1);
    if (r === 0) return null;
    if (b[0] === 10) return s;
    s += b.toString("utf8");
  }
}
function send(ok, text) {
  const data = Buffer.from(String(text), "utf8");
  fs.writeSync(1, ok ? "ok\n" : "err\n");
  fs.writeSync(1, String(data.length) + "\n");
  fs.writeSync(1, data);
}
for (;;) {
  const header = readLine();
  if (header === null) break;
  const n = parseInt(header, 10);
  if (!Number.isFinite(n) || n < 0) { send(false, "invalid frame header"); continue; }
  const buf = readExact(n);
  if (buf === null) break;
  const code = buf.toString("utf8");
  let out = "";
  const origLog = console.log;
  const origErr = console.error;
  const origWarn = console.warn;
  const capture = (...args) => {
    out += args.map(a => typeof a === "string" ? a : util.inspect(a)).join(" ") + "\n";
  };
  console.log = capture;
  console.error = capture;
  console.warn = capture;
  let ok = true;
  try {
    const result = vm.runInContext(code, context, { filename: "repl.js" });
    if (result !== undefined) {
      out += util.inspect(result) + "\n";
    }
  } catch (e) {
    ok = false;
    out += (e && e.stack) ? e.stack + "\n" : String(e) + "\n";
  } finally {
    console.log = origLog;
    console.error = origErr;
    console.warn = origWarn;
  }
  send(ok, out);
}
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Language {
    Python,
    Node,
}

impl Language {
    fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "python" | "py" | "python3" => Ok(Self::Python),
            "node" | "js" | "javascript" => Ok(Self::Node),
            other => bail!("unsupported language '{other}' (use python or node)"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Node => "node",
        }
    }
}

enum SessionBackend {
    Supervised { child_id: String },
    Direct { child: Child },
}

struct ReplSession {
    backend: SessionBackend,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Resolved interpreter binary used to spawn this session.
    interpreter: String,
    spawned_at: Instant,
    pid: Option<u32>,
}

/// Stateful Python/Node REPL tool.
pub struct ReplTool {
    supervisor: Option<Arc<ParkingMutex<ProcessSupervisor>>>,
    default_working_dir: Option<String>,
    sessions: Arc<AsyncMutex<HashMap<Language, ReplSession>>>,
}

impl Default for ReplTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplTool {
    pub fn new() -> Self {
        Self {
            supervisor: None,
            default_working_dir: None,
            sessions: Arc::new(AsyncMutex::new(HashMap::new())),
        }
    }

    pub fn with_supervisor(
        supervisor: Arc<ParkingMutex<ProcessSupervisor>>,
        default_working_dir: Option<String>,
    ) -> Self {
        Self {
            supervisor: Some(supervisor),
            default_working_dir,
            sessions: Arc::new(AsyncMutex::new(HashMap::new())),
        }
    }

    pub fn with_default_working_dir(default_working_dir: Option<String>) -> Self {
        Self {
            supervisor: None,
            default_working_dir,
            sessions: Arc::new(AsyncMutex::new(HashMap::new())),
        }
    }

    fn working_dir(&self) -> String {
        self.default_working_dir
            .clone()
            .unwrap_or_else(|| ".".to_string())
    }
}

#[async_trait]
impl Tool for ReplTool {
    fn name(&self) -> &str {
        "repl"
    }

    fn description(&self) -> &str {
        "Stateful Python/Node scratchpad. Bindings persist across calls in this run. \
         Prefer over shell for exploratory Python/JS (probing APIs, shaping data, trying snippets). \
         \
         Environment workflow (act like a careful human — do NOT rush to install packages): \
         1) Discover an existing project env first (.venv, uv/.venv, conda env, node from nvm/fnm). \
         2) Pass its binary via `interpreter` (e.g. `.venv/bin/python`, conda env's python, or an absolute node path). \
         3) Inspect what is already installed BEFORE installing anything — e.g. Python: \
         `import importlib.metadata as m; sorted(d.metadata['Name'] for d in m.distributions())` \
         or shell `…/python -m pip list` / `uv pip list`; Node: `require.resolve` / package.json deps. \
         4) Only use shell to install (`uv add` / `pip install` / `npm i`) if a needed package is truly missing. \
         Prefer reusing the env's packages over creating a new env. \
         \
         Changing `interpreter` resets that language's session. Use shell for builds and package installs, \
         not for exploratory eval. Node: prefer `var x=…` or `globalThis.x=…` (top-level let/const do not persist). \
         Actions: exec (default), reset, status. Timeout kills that language session."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["python", "node"],
                    "description": "Language: python or node"
                },
                "interpreter": {
                    "type": "string",
                    "description": "Optional path to the python/node binary for this session \
(e.g. .venv/bin/python, /path/to/conda/envs/foo/bin/python, or a specific node). \
Omit to use PATH default (python3/python or node). Changing this resets the session."
                },
                "code": {
                    "type": "string",
                    "description": "Code to execute (required for action=exec)"
                },
                "action": {
                    "type": "string",
                    "enum": ["exec", "reset", "status"],
                    "description": "exec (default), reset session, or status"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Per-eval timeout in seconds (default: 30)"
                }
            },
            "required": ["language"]
        })
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Sequential)
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.execute_with_stream(args, None, None).await
    }

    async fn execute_with_stream(
        &self,
        args: Value,
        on_update: Option<ToolUpdateFn>,
        _event_sender: Option<EventSender>,
    ) -> Result<String> {
        let language = Language::parse(
            args.get("language")
                .and_then(|v| v.as_str())
                .context("missing 'language'")?,
        )?;
        let interpreter = args
            .get("interpreter")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("exec");
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        match action {
            "status" => self.status(language).await,
            "reset" => self.reset(language).await,
            "exec" => {
                let code = args
                    .get("code")
                    .and_then(|v| v.as_str())
                    .context("missing 'code' for action=exec")?;
                self.exec(language, interpreter.as_deref(), code, timeout_secs, on_update)
                    .await
            }
            other => bail!("unknown action '{other}' (use exec, reset, or status)"),
        }
    }
}

impl ReplTool {
    async fn status(&self, language: Language) -> Result<String> {
        let sessions = self.sessions.lock().await;
        match sessions.get(&language) {
            Some(s) => Ok(format!(
                "language={} alive=true interpreter={} pid={} uptime_secs={}",
                language.as_str(),
                s.interpreter,
                s.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
                s.spawned_at.elapsed().as_secs()
            )),
            None => Ok(format!(
                "language={} alive=false",
                language.as_str()
            )),
        }
    }

    async fn reset(&self, language: Language) -> Result<String> {
        self.kill_session(language).await?;
        Ok(format!("reset {}", language.as_str()))
    }

    async fn exec(
        &self,
        language: Language,
        interpreter: Option<&str>,
        code: &str,
        timeout_secs: u64,
        on_update: Option<ToolUpdateFn>,
    ) -> Result<String> {
        let cwd = self.working_dir();
        let resolved = resolve_interpreter(&cwd, language, interpreter)?;

        // Ensure session exists and matches the requested interpreter.
        let needs_spawn = {
            let sessions = self.sessions.lock().await;
            match sessions.get(&language) {
                None => true,
                Some(s) if s.interpreter != resolved => true,
                Some(_) => false,
            }
        };
        if needs_spawn {
            self.kill_session(language).await?;
            let session = self.spawn_session(language, &resolved).await?;
            self.sessions.lock().await.insert(language, session);
        }

        let result = {
            let mut sessions = self.sessions.lock().await;
            let session = sessions
                .get_mut(&language)
                .ok_or_else(|| anyhow::anyhow!("repl session missing after spawn"))?;

            let fut = eval_in_session(session, code, on_update.as_ref());
            tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await
        };

        match result {
            Ok(Ok((ok, output))) => {
                let mut body = truncate_output(output);
                if !ok {
                    body.push_str("\n[repl error]");
                }
                Ok(body)
            }
            Ok(Err(e)) => {
                let _ = self.kill_session(language).await;
                Err(e)
            }
            Err(_elapsed) => {
                let _ = self.kill_session(language).await;
                bail!(
                    "repl timed out after {}s; {} session was reset",
                    timeout_secs,
                    language.as_str()
                )
            }
        }
    }

    async fn kill_session(&self, language: Language) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        if let Some(mut session) = sessions.remove(&language) {
            match session.backend {
                SessionBackend::Supervised { child_id } => {
                    if let Some(ref sup) = self.supervisor {
                        let mut supervisor = sup.lock();
                        let _ = supervisor.kill(&child_id);
                    }
                }
                SessionBackend::Direct { ref mut child } => {
                    let _ = child.start_kill();
                    let _ = child.try_wait();
                }
            }
            drop(session.stdin);
            drop(session.stdout);
        }
        Ok(())
    }

    async fn spawn_session(&self, language: Language, interpreter: &str) -> Result<ReplSession> {
        let cwd = self.working_dir();
        let args = bootstrap_args(language);

        if let Some(ref sup) = self.supervisor {
            let child_id = {
                let mut supervisor = sup.lock();
                supervisor.spawn_process(interpreter, &args, Some(&cwd))?
            };

            let (stdin, stdout, pid) = {
                let mut supervisor = sup.lock();
                let child = supervisor
                    .get_child(&child_id)
                    .ok_or_else(|| anyhow::anyhow!("repl child disappeared after spawn"))?;
                let stdin = child
                    .take_stdin()
                    .ok_or_else(|| anyhow::anyhow!("repl stdin not piped"))?;
                let stdout = child
                    .take_stdout()
                    .ok_or_else(|| anyhow::anyhow!("repl stdout not piped"))?;
                if let Some(stderr) = child.take_stderr() {
                    tokio::spawn(async move {
                        let mut stderr = stderr;
                        let mut buf = vec![0u8; 4096];
                        loop {
                            match tokio::io::AsyncReadExt::read(&mut stderr, &mut buf).await {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {}
                            }
                        }
                    });
                }
                let pid = child.pid();
                (stdin, stdout, pid)
            };

            return Ok(ReplSession {
                backend: SessionBackend::Supervised { child_id },
                stdin,
                stdout: BufReader::new(stdout),
                interpreter: interpreter.to_string(),
                spawned_at: Instant::now(),
                pid,
            });
        }

        let mut cmd = tokio::process::Command::new(interpreter);
        cmd.args(&args)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().with_context(|| {
            format!("{interpreter} not found or failed to start")
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("repl stdin not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("repl stdout not piped"))?;
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                loop {
                    match tokio::io::AsyncReadExt::read(&mut stderr, &mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
        }
        let pid = child.id();

        Ok(ReplSession {
            backend: SessionBackend::Direct { child },
            stdin,
            stdout: BufReader::new(stdout),
            interpreter: interpreter.to_string(),
            spawned_at: Instant::now(),
            pid,
        })
    }
}

async fn eval_in_session(
    session: &mut ReplSession,
    code: &str,
    on_update: Option<&ToolUpdateFn>,
) -> Result<(bool, String)> {
    let bytes = code.as_bytes();
    session
        .stdin
        .write_all(format!("{}\n", bytes.len()).as_bytes())
        .await
        .context("failed to write repl frame header")?;
    session
        .stdin
        .write_all(bytes)
        .await
        .context("failed to write repl code")?;
    session
        .stdin
        .flush()
        .await
        .context("failed to flush repl stdin")?;

    let mut status_line = String::new();
    session
        .stdout
        .read_line(&mut status_line)
        .await
        .context("failed to read repl status")?;
    if status_line.is_empty() {
        bail!("repl process closed unexpectedly");
    }
    let ok = match status_line.trim() {
        "ok" => true,
        "err" => false,
        other => bail!("invalid repl status '{other}'"),
    };

    let mut len_line = String::new();
    session
        .stdout
        .read_line(&mut len_line)
        .await
        .context("failed to read repl length")?;
    let len: usize = len_line
        .trim()
        .parse()
        .with_context(|| format!("invalid repl length '{}'", len_line.trim()))?;

    let mut buf = vec![0u8; len];
    if len > 0 {
        session
            .stdout
            .read_exact(&mut buf)
            .await
            .context("failed to read repl body")?;
    }
    let output = String::from_utf8_lossy(&buf).to_string();
    if let Some(cb) = on_update {
        if !output.is_empty() {
            cb(&output);
        }
    }
    Ok((ok, output))
}

fn truncate_output(mut s: String) -> String {
    if s.chars().count() > MAX_OUTPUT_CHARS {
        let truncated: String = s.chars().take(MAX_OUTPUT_CHARS).collect();
        s = truncated;
        s.push_str("\n…[output truncated]…\n");
    }
    s
}

fn bootstrap_args(language: Language) -> Vec<String> {
    match language {
        Language::Python => vec![
            "-u".into(),
            "-c".into(),
            PYTHON_BOOTSTRAP.trim().into(),
        ],
        Language::Node => vec!["-e".into(), NODE_BOOTSTRAP.trim().into()],
    }
}

/// Resolve the interpreter binary path.
///
/// - If `interpreter` looks like a filesystem path (absolute, relative `./…`,
///   or contains a separator): resolve against `working_dir` if needed and
///   require the file to exist.
/// - If `interpreter` is a bare name (`python3`, `node`): use it as a PATH
///   program (same as default lookup).
/// - If omitted: fall back to PATH (`python3`/`python` or `node`).
fn resolve_interpreter(
    working_dir: &str,
    language: Language,
    interpreter: Option<&str>,
) -> Result<String> {
    if let Some(raw) = interpreter {
        let looks_like_path = raw.starts_with('.')
            || raw.starts_with('/')
            || raw.starts_with('~')
            || raw.contains('/')
            || raw.contains('\\')
            || (raw.len() >= 3
                && raw.as_bytes()[1] == b':'
                && (raw.as_bytes()[2] == b'\\' || raw.as_bytes()[2] == b'/'));

        let program = if looks_like_path {
            let path = std::path::PathBuf::from(raw);
            let abs = if path.is_absolute() {
                path
            } else if let Some(stripped) = raw.strip_prefix("~/") {
                let home = std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(std::path::PathBuf::from)
                    .ok_or_else(|| anyhow::anyhow!("cannot expand ~ in interpreter path"))?;
                home.join(stripped)
            } else {
                std::path::PathBuf::from(working_dir).join(path)
            };
            if !abs.exists() {
                bail!(
                    "interpreter not found: {} (discover .venv/conda/uv env first, then pass its python/node binary)",
                    abs.display()
                );
            }
            abs.to_string_lossy().to_string()
        } else {
            raw.to_string()
        };

        let status = std::process::Command::new(&program)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            bail!("interpreter is not runnable: {program} (`--version` failed)");
        }
        return Ok(program);
    }

    match language {
        Language::Python => resolve_binary(&["python3", "python"])
            .context("python3/python not found on PATH (pass interpreter= to a venv/conda/uv python)"),
        Language::Node => resolve_binary(&["node"])
            .context("node not found on PATH (pass interpreter= to a specific node binary)"),
    }
}

fn resolve_binary(candidates: &[&str]) -> Option<String> {
    for name in candidates {
        let status = std::process::Command::new(name)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if matches!(status, Ok(s) if s.success()) {
            return Some((*name).to_string());
        }
    }
    None
}

/// Fingerprint for permission scoping: `language:interpreter:action:code`.
pub fn repl_permission_fingerprint(args: &Value) -> Option<String> {
    let language = args.get("language").and_then(|v| v.as_str())?;
    let interpreter = args
        .get("interpreter")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("exec");
    let code = args.get("code").and_then(|v| v.as_str()).unwrap_or("");
    Some(format!("{language}:{interpreter}:{action}:{code}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool() -> ReplTool {
        ReplTool::new()
    }

    fn python_available() -> bool {
        resolve_binary(&["python3", "python"]).is_some()
    }

    fn node_available() -> bool {
        resolve_binary(&["node"]).is_some()
    }

    #[tokio::test]
    async fn python_retains_bindings_across_calls() {
        if !python_available() {
            eprintln!("skip: no python");
            return;
        }
        let t = tool();
        t.execute(json!({"language": "python", "code": "x = 1"}))
            .await
            .unwrap();
        let out = t
            .execute(json!({"language": "python", "code": "x + 1"}))
            .await
            .unwrap();
        assert!(out.contains('2'), "got: {out}");
    }

    #[tokio::test]
    async fn node_retains_bindings_via_global_this() {
        if !node_available() {
            eprintln!("skip: no node");
            return;
        }
        let t = tool();
        t.execute(json!({
            "language": "node",
            "code": "globalThis.x = 1"
        }))
        .await
        .unwrap();
        let out = t
            .execute(json!({
                "language": "node",
                "code": "globalThis.x + 1"
            }))
            .await
            .unwrap();
        assert!(out.contains('2'), "got: {out}");
    }

    #[tokio::test]
    async fn reset_clears_python_bindings() {
        if !python_available() {
            eprintln!("skip: no python");
            return;
        }
        let t = tool();
        t.execute(json!({"language": "python", "code": "x = 99"}))
            .await
            .unwrap();
        t.execute(json!({"language": "python", "action": "reset"}))
            .await
            .unwrap();
        let out = t
            .execute(json!({"language": "python", "code": "x"}))
            .await
            .unwrap();
        assert!(
            out.contains("[repl error]") || out.to_lowercase().contains("nameerror"),
            "expected NameError after reset, got: {out}"
        );
    }

    #[tokio::test]
    async fn timeout_resets_session() {
        if !python_available() {
            eprintln!("skip: no python");
            return;
        }
        let t = tool();
        let err = t
            .execute(json!({
                "language": "python",
                "code": "import time; time.sleep(60)",
                "timeout_secs": 1
            }))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("timed out"),
            "got: {err}"
        );
        // Fresh session should work
        let out = t
            .execute(json!({"language": "python", "code": "1 + 1"}))
            .await
            .unwrap();
        assert!(out.contains('2'), "got: {out}");
    }

    #[tokio::test]
    async fn status_reports_interpreter() {
        if !python_available() {
            eprintln!("skip: no python");
            return;
        }
        let t = tool();
        t.execute(json!({"language": "python", "code": "1"}))
            .await
            .unwrap();
        let after = t
            .execute(json!({"language": "python", "action": "status"}))
            .await
            .unwrap();
        assert!(after.contains("alive=true"), "got: {after}");
        assert!(after.contains("interpreter="), "got: {after}");
    }

    #[tokio::test]
    async fn custom_interpreter_path_works() {
        if !python_available() {
            eprintln!("skip: no python");
            return;
        }
        // Resolve PATH python to an absolute-ish name and pass it explicitly.
        let py = resolve_binary(&["python3", "python"]).unwrap();
        let t = tool();
        let out = t
            .execute(json!({
                "language": "python",
                "interpreter": py,
                "code": "import sys; sys.executable"
            }))
            .await
            .unwrap();
        assert!(!out.contains("[repl error]"), "got: {out}");
        let status = t
            .execute(json!({"language": "python", "action": "status"}))
            .await
            .unwrap();
        assert!(status.contains(&py), "status should show interpreter, got: {status}");
    }

    #[tokio::test]
    async fn missing_interpreter_path_is_actionable() {
        let t = tool();
        let err = t
            .execute(json!({
                "language": "python",
                "interpreter": "/nonexistent/agverse-venv/bin/python",
                "code": "1"
            }))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("interpreter not found"), "got: {msg}");
    }

    #[tokio::test]
    async fn missing_binary_is_actionable() {
        assert!(resolve_binary(&["definitely-not-a-real-bin-agverse-xyz"]).is_none());
    }

    #[test]
    fn fingerprint_includes_interpreter() {
        let fp = repl_permission_fingerprint(&json!({
            "language": "python",
            "interpreter": ".venv/bin/python",
            "action": "exec",
            "code": "print(1)"
        }))
        .unwrap();
        assert_eq!(fp, "python:.venv/bin/python:exec:print(1)");
    }

    #[test]
    fn fingerprint_includes_language_action_code() {
        let fp = repl_permission_fingerprint(&json!({
            "language": "python",
            "action": "exec",
            "code": "print(1)"
        }))
        .unwrap();
        assert_eq!(fp, "python::exec:print(1)");
    }
}
