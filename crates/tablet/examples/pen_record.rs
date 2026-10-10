//! Record pen tablet input on X11 (Xwayland included) to a CSV, whatever app the pen draws in, so
//! another painting app's result can be compared with PhotoCraft fed the very same pen input:
//!
//! ```sh
//! cargo run --release -p photocraft-tablet --example pen_record -- pen.csv
//! ```
//!
//! Rows use `PHOTOCRAFT_PEN_TRACE`'s format (`t_ms,kind,…`): `dev` (device list), `raw` (every
//! raw XInput2 event: server time, device, kind, button, valuators) and `sample` (the mapped pen
//! sample). It runs until killed (Ctrl+C); every row is flushed as it is written. Once a second it
//! prints how many pen events it has seen, so you can tell it is capturing.

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    use std::fs::File;
    use std::io::{LineWriter, Write};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use photocraft_tablet::x11::{Raw, RawKind};

    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: pen_record <out.csv>");
        return std::process::ExitCode::FAILURE;
    };
    let file = match File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot create {path}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let out = Arc::new(Mutex::new(LineWriter::new(file)));
    let origin = Instant::now();
    let row = {
        let out = out.clone();
        move |kind: &str, fields: String| {
            let t = origin.elapsed().as_secs_f64() * 1000.0;
            let _ = writeln!(out.lock().unwrap_or_else(PoisonError::into_inner), "{t:.3},{kind},{fields}");
        }
    };
    let _ = writeln!(out.lock().unwrap_or_else(PoisonError::into_inner), "t_ms,kind,fields…");
    let field = |s: &str| s.chars().map(|c| if matches!(c, ',' | '"' | '\n' | '\r') { ';' } else { c }).collect::<String>();
    let pen_events = Arc::new(AtomicU64::new(0));
    let sample_row = row.clone();
    let counted = pen_events.clone();
    let callback = move |s: Option<photocraft_tablet::Sample>| {
        if let Some(p) = s {
            counted.fetch_add(1, Ordering::Relaxed);
            sample_row("sample", format!("{:.5},{:.2},{:.2},{:.2},{}", p.pressure, p.tilt_x, p.tilt_y, p.rotation, u8::from(p.eraser)));
        } else {
            sample_row("sample", ",,,,".into());
        }
    };
    let trace: photocraft_tablet::x11::Trace = Box::new(move |raw: &Raw| match raw {
        Raw::Devices(devices) => {
            for d in devices {
                let axes: Vec<String> = d.valuators.iter().map(|(n, label, min, max)| format!("{n}={}={min}={max}", field(label))).collect();
                row("dev", format!("{},{},{}", d.id, field(&d.name), axes.join("|")));
            }
        }
        Raw::Event { time_ms, device, kind, button, valuators } => {
            let kind = match kind {
                RawKind::Motion => "motion",
                RawKind::Press => "press",
                RawKind::Release => "release",
            };
            let values: Vec<String> = valuators.iter().map(|(n, v)| format!("{n}={v}")).collect();
            row("raw", format!("{time_ms},{device},{kind},{button},{}", values.join("|")));
        }
    });
    if let Err(e) = photocraft_tablet::x11::spawn_traced(None, callback, Some(trace)) {
        eprintln!("{e}");
        return std::process::ExitCode::FAILURE;
    }
    eprintln!("recording pen input to {path} (Ctrl+C to stop)");
    loop {
        std::thread::sleep(Duration::from_secs(1));
        eprintln!("pen events: {}", pen_events.load(Ordering::Relaxed));
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("pen_record reads X11 tablet input: Linux only");
}
