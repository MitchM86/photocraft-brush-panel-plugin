//! Opt-in recording of pen input, for diagnosing pressure: `PHOTOCRAFT_PEN_TRACE=<file.csv>`
//! makes every stage from the tablet to the brush write a row to that file, on one clock.
//! Unset (the default), nothing is collected or written.
//!
//! Each row is `t_ms,kind,…` (`t_ms`: milliseconds since the trace opened, µs resolution); the
//! fields after `kind` depend on it:
//!
//! | kind | fields | written by |
//! |---|---|---|
//! | `dev` | `id,name,valuators` (`number=label=min=max`, `\|`-separated) | X11 reader: device list |
//! | `raw` | `server_ms,device,motion\|press\|release,button,valuators` (`number=value`, `\|`-separated) | X11 reader: every raw event |
//! | `sample` | `pressure,tilt_x,tilt_y,rotation,eraser` (empty fields: a mouse) | the tablet sink, as the feed gets it |
//! | `frame` | `drained,kept,touch` | [`crate::stylus::Stylus::update`]: the frame's pen samples |
//! | `select` | `k,n,m,i,t,pressure_a,pressure_b,pressure` | [`crate::stylus::Stylus::select`]: interpolation for move `k` of `n` |
//! | `move` | `k,n,screen_x,screen_y,doc_x,doc_y,pressure` | the canvas: each pointer move it hands the tool |
//! | `down` / `up` | `x,y,pressure` / `x,y,points` | the tool state machine |
//! | `point` | `x,y,pressure,tilt_x,tilt_y,rotation` | a point added to the drag (what `paint.stroke` gets) |
//! | `brush` | the brush settings as JSON (to the end of the line) | each stroke's start |
//!
//! Free text (device names) has commas and line breaks replaced, so every row splits on `,`
//! except `brush`, whose JSON is the rest of the line.

/// The environment variable naming the trace file.
pub const ENV: &str = "PHOTOCRAFT_PEN_TRACE";

/// Is a trace being written? Cheap; check it before collecting anything expensive.
pub fn enabled() -> bool {
    imp::enabled()
}

/// Write one row of `kind` with its `fields` (already comma-separated). No-op when off.
pub fn row(kind: &str, fields: std::fmt::Arguments<'_>) {
    imp::row(kind, fields);
}

/// Free text as one CSV field: no commas, quotes or line breaks.
pub fn field(s: &str) -> String {
    s.chars().map(|c| if matches!(c, ',' | '"' | '\n' | '\r') { ';' } else { c }).collect()
}

/// Formats rows onto a writer, timed from when it was made.
#[cfg(not(target_arch = "wasm32"))]
struct Writer<W: std::io::Write> {
    origin: std::time::Instant,
    out: W,
}

#[cfg(not(target_arch = "wasm32"))]
impl<W: std::io::Write> Writer<W> {
    fn new(mut out: W) -> Self {
        let _ = writeln!(out, "t_ms,kind,fields…");
        Self { origin: std::time::Instant::now(), out }
    }

    fn row(&mut self, kind: &str, fields: std::fmt::Arguments<'_>) {
        let t = self.origin.elapsed().as_secs_f64() * 1000.0;
        // A full disk or a closed file loses trace rows, never input.
        let _ = writeln!(self.out, "{t:.3},{kind},{fields}");
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::fs::File;
    use std::io::LineWriter;
    use std::sync::{Mutex, OnceLock, PoisonError};

    use super::Writer;

    /// Opened on first use from [`super::ENV`]; `None` when unset or the file can't be created.
    static TRACE: OnceLock<Option<Mutex<Writer<LineWriter<File>>>>> = OnceLock::new();

    fn trace() -> Option<&'static Mutex<Writer<LineWriter<File>>>> {
        TRACE.get_or_init(open).as_ref()
    }

    fn open() -> Option<Mutex<Writer<LineWriter<File>>>> {
        let path = std::env::var_os(super::ENV).filter(|p| !p.is_empty())?;
        match File::create(&path) {
            Ok(f) => {
                log::info!("pen trace: writing {}", std::path::Path::new(&path).display());
                // Line-buffered, so a crash or a killed app keeps everything written so far.
                Some(Mutex::new(Writer::new(LineWriter::new(f))))
            }
            Err(e) => {
                log::warn!("pen trace: cannot create {}: {e}", std::path::Path::new(&path).display());
                None
            }
        }
    }

    pub fn enabled() -> bool {
        trace().is_some()
    }

    pub fn row(kind: &str, fields: std::fmt::Arguments<'_>) {
        if let Some(t) = trace() {
            t.lock().unwrap_or_else(PoisonError::into_inner).row(kind, fields);
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    pub fn enabled() -> bool {
        false
    }

    pub fn row(_kind: &str, _fields: std::fmt::Arguments<'_>) {}
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn rows_are_timed_and_comma_separated() {
        let mut w = Writer::new(Vec::new());
        w.row("move", format_args!("{},{},{:.4}", 0, 3, 0.25));
        w.row("dev", format_args!("{},{}", 9, field("Pen, \"stylus\"\nx")));
        let text = String::from_utf8(w.out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].starts_with("t_ms,kind"));
        let cols: Vec<&str> = lines[1].split(',').collect();
        assert!(cols[0].parse::<f64>().is_ok_and(|t| t >= 0.0), "{cols:?}");
        assert_eq!(&cols[1..], ["move", "0", "3", "0.2500"]);
        assert!(lines[2].ends_with(",dev,9,Pen; ;stylus;;x"), "{}", lines[2]);
    }

    #[test]
    fn off_without_the_variable() {
        // The test runner doesn't set it; rows are then dropped without error.
        if std::env::var_os(ENV).is_none() {
            assert!(!enabled());
            row("move", format_args!("1"));
        }
    }
}
