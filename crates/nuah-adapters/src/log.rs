//! Log Adapters: the console (pretty on a terminal, JSON lines elsewhere) and a rotating file.
//! They only decide how a record looks and where it is written; what is recorded is the Core's business.

use nuah_kernel::*;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::io::{IsTerminal, Write};
use std::sync::Arc;

/// `2026-11-07T18:31:43.707Z` from milliseconds since the epoch (UTC), without a date library.
pub fn iso(ms: i64) -> String {
    let (secs, milli) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// One record as one JSON object on one line.
pub fn json_line(r: &Record) -> String {
    let mut o = json!({"time": iso(r.time), "level": r.level.as_str(), "target": r.target, "key": r.key});
    if !r.fields.is_null() {
        o["fields"] = r.fields.clone();
    }
    if let Some(e) = &r.error {
        o["error"] = serde_json::to_value(e).unwrap_or(Value::Null);
    }
    if let Some(q) = &r.request {
        o["request"] = json!(q);
    }
    o.to_string()
}

fn fields_text(v: &Value) -> String {
    match v {
        Value::Object(o) => o
            .iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| match v {
                Value::String(s) => format!("{k}={s}"),
                Value::Number(n) if n.is_f64() => format!("{k}={:.1}", n.as_f64().unwrap_or(0.0)),
                v => format!("{k}={v}"),
            })
            .collect::<Vec<_>>()
            .join(" "),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// One record for a person to read.
pub fn pretty_line(r: &Record, color: bool) -> String {
    let (code, name) = match r.level {
        Level::Error => ("31", "ERROR"),
        Level::Warn => ("33", "WARN "),
        Level::Info => ("32", "INFO "),
        Level::Debug => ("36", "DEBUG"),
        Level::Trace => ("90", "TRACE"),
    };
    let paint = |c: &str, s: &str| if color { format!("\x1b[{c}m{s}\x1b[0m") } else { s.to_string() };
    let mut s = format!("{} {} {} {}", paint("90", &iso(r.time)), paint(code, name), paint("1", &r.target), r.key);
    let f = fields_text(&r.fields);
    if !f.is_empty() {
        s.push(' ');
        s.push_str(&f);
    }
    if let Some(e) = &r.error {
        s.push_str(&format!(" {} {} ({})", paint("31", "error:"), e.message, e.category));
        if let Some(x) = &e.reference {
            s.push_str(&format!(" ref={x}"));
        }
    }
    s
}

/// The Adapter's own minimum, which only narrows what the Core's level lets through.
fn level_of(settings: &Value, default: Level) -> Level {
    settings.get("level").and_then(|l| l.as_str()).and_then(Level::parse).unwrap_or(default)
}

// ---- console ---------------------------------------------------------------------------------------------

pub struct ConsoleLog {
    min: Level,
    json: bool,
    color: bool,
    stdout: bool,
}

impl LogAdapter for ConsoleLog {
    fn name(&self) -> &str {
        "log-console"
    }
    fn min_level(&self) -> Level {
        self.min
    }
    fn record(&self, r: &Record) {
        let line = if self.json { json_line(r) } else { pretty_line(r, self.color) };
        if self.stdout {
            let _ = writeln!(std::io::stdout().lock(), "{line}");
        } else {
            let _ = writeln!(std::io::stderr().lock(), "{line}");
        }
    }
    fn flush(&self) {
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
    }
}

pub struct LogConsole;
impl Plugin for LogConsole {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id": "log-console", "version": "0.1.0", "name": "Console log", "fills": "log",
            "description": "Writes log records to the terminal: readable and coloured on a terminal, one JSON object per line otherwise",
            "settings_schema": {"type": "object", "additionalProperties": false, "properties": {
                "level": {"type": "string", "enum": ["error", "warn", "info", "debug", "trace"], "x-tier": "operator", "x-apply": "restart", "description": "Records chattier than this are not written here"},
                "format": {"type": "string", "enum": ["auto", "pretty", "json"], "x-tier": "operator", "x-apply": "restart", "description": "auto: pretty on a terminal, JSON otherwise (the right default in a container)"},
                "color": {"type": "string", "enum": ["auto", "always", "never"], "x-tier": "operator", "x-apply": "restart"},
                "stream": {"type": "string", "enum": ["stderr", "stdout"], "x-tier": "operator", "x-apply": "restart"}
            }}
        })).unwrap()
    }
    fn fill(&self, s: &Value) -> Result<Option<PortImpl>> {
        let stdout = s.get("stream").and_then(|v| v.as_str()) == Some("stdout");
        let tty = if stdout { std::io::stdout().is_terminal() } else { std::io::stderr().is_terminal() };
        let json = match s.get("format").and_then(|v| v.as_str()).unwrap_or("auto") {
            "json" => true,
            "pretty" => false,
            _ => !tty,
        };
        let color = match s.get("color").and_then(|v| v.as_str()).unwrap_or("auto") {
            "always" => true,
            "never" => false,
            _ => tty && std::env::var_os("NO_COLOR").is_none(),
        };
        Ok(Some(PortImpl::Log(Arc::new(ConsoleLog { min: level_of(s, Level::Trace), json, color, stdout }))))
    }
}

// ---- file ------------------------------------------------------------------------------------------------

struct FileState {
    file: std::fs::File,
    size: u64,
}

pub struct FileLog {
    path: std::path::PathBuf,
    min: Level,
    max_bytes: u64,
    keep: usize,
    state: Mutex<FileState>,
}

impl FileLog {
    fn open(path: &std::path::Path) -> std::io::Result<FileState> {
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(FileState { file, size })
    }
    fn rotate(&self, st: &mut FileState) {
        let _ = st.file.flush();
        for i in (1..self.keep).rev() {
            let _ = std::fs::rename(self.numbered(i), self.numbered(i + 1));
        }
        if self.keep > 0 {
            let _ = std::fs::rename(&self.path, self.numbered(1));
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
        if let Ok(s) = Self::open(&self.path) {
            *st = s;
        }
    }
    fn numbered(&self, i: usize) -> std::path::PathBuf {
        let mut n = self.path.as_os_str().to_owned();
        n.push(format!(".{i}"));
        n.into()
    }
}

impl LogAdapter for FileLog {
    fn name(&self) -> &str {
        "log-file"
    }
    fn min_level(&self) -> Level {
        self.min
    }
    fn record(&self, r: &Record) {
        let line = json_line(r) + "\n";
        let mut st = self.state.lock();
        if st.size > 0 && st.size + line.len() as u64 > self.max_bytes {
            self.rotate(&mut st);
        }
        if st.file.write_all(line.as_bytes()).is_ok() {
            st.size += line.len() as u64;
        }
    }
    fn flush(&self) {
        let _ = self.state.lock().file.flush();
    }
}

pub struct LogFile;
impl Plugin for LogFile {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id": "log-file", "version": "0.1.0", "name": "File log", "fills": "log",
            "description": "Appends log records to a file as JSON lines, and rotates it when it grows",
            "settings_schema": {"type": "object", "additionalProperties": false, "required": ["path"], "properties": {
                "path": {"type": "string", "minLength": 1, "x-tier": "bootstrap", "x-apply": "restart"},
                "level": {"type": "string", "enum": ["error", "warn", "info", "debug", "trace"], "x-tier": "operator", "x-apply": "restart"},
                "max_bytes": {"type": "integer", "minimum": 1024, "x-tier": "operator", "x-apply": "restart", "description": "Rotate when the file would grow past this (10 MiB when absent)"},
                "keep": {"type": "integer", "minimum": 0, "maximum": 100, "x-tier": "operator", "x-apply": "restart", "description": "Rotated files kept (5 when absent)"}
            }}
        })).unwrap()
    }
    fn fill(&self, s: &Value) -> Result<Option<PortImpl>> {
        let path = std::path::PathBuf::from(s.get("path").and_then(|p| p.as_str()).unwrap_or("nuah.log"));
        let state = FileLog::open(&path).map_err(|e| Error::internal(format!("cannot open the log file {}: {e}", path.display())))?;
        Ok(Some(PortImpl::Log(Arc::new(FileLog {
            path,
            min: level_of(s, Level::Trace),
            max_bytes: s.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(10 * 1024 * 1024),
            keep: s.get("keep").and_then(|v| v.as_u64()).unwrap_or(5) as usize,
            state: Mutex::new(state),
        }))))
    }
}
