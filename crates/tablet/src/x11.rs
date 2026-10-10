//! XInput2 raw tablet events on X11, over a separate pure-Rust X connection (x11rb's
//! `RustConnection`, no `unsafe`, no libxcb).
//!
//! winit selects XI2 pointer events on its window for the master pointer, whose events carry the
//! valuators of whichever slave moved but whose valuator layout changes with it, and which go to
//! winit alone while a drag holds the implicit grab. Raw events (`XI_RawMotion`,
//! `XI_RawButtonPress/Release`) selected on the root window have neither problem: since XI 2.1
//! every client that selects them gets them, grabbed or not, tagged with the slave device. So
//! this thread listens to those, plus hierarchy and device changes to keep the device list
//! current, and maps them with [`crate::xi`].
//!
//! Raw events don't say which window the pen is over. That's fine for pressure: the UI only uses
//! the sample while its own canvas receives pointer events, and any other pointer moving resets
//! it to "mouse".

use std::thread::JoinHandle;

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{self, ConnectionExt as _, DeviceClassData, DeviceType, XIEventMask};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::xi::{Device, State, fp3232, valuators};
use crate::{Error, Sample, Update, deliver};

/// `XIAllDevices`: select for every device, slaves included.
const ALL_DEVICES: u16 = 0;

/// A slave pointer device as `XIQueryDevice` reports it, every valuator kept (for [`Raw`]).
#[derive(Clone, Debug, PartialEq)]
pub struct RawDevice {
    pub id: u16,
    pub name: String,
    /// Each valuator's (number, label, min, max).
    pub valuators: Vec<(u16, String, f64, f64)>,
}

/// Which raw event a [`Raw::Event`] was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawKind {
    Motion,
    Press,
    Release,
}

/// What the server sent, before any mapping: for diagnosing pen input ([`spawn_traced`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Raw {
    /// The device list, read on start and on every hierarchy or device change.
    Devices(Vec<RawDevice>),
    /// A raw event: server time in milliseconds, slave device, button (`detail`, 0 for motion)
    /// and the valuators it carries (all of them, not only the pen axes).
    Event { time_ms: u32, device: u16, kind: RawKind, button: u32, valuators: Vec<(u16, f64)> },
}

/// Diagnostics callback of [`spawn_traced`].
pub type Trace = Box<dyn Fn(&Raw) + Send + 'static>;

/// Start listening on `display` (`None` = `$DISPLAY`, the display winit uses). The connection
/// and the XI2 version are checked before this returns; events are then read on a background
/// thread that calls `callback` with `Some(sample)` for pen events and `None` when another
/// pointer moves. The thread ends when the X connection closes.
pub fn spawn(display: Option<&str>, callback: impl Fn(Option<Sample>) + Send + 'static) -> Result<JoinHandle<()>, Error> {
    spawn_traced(display, callback, None)
}

/// [`spawn`], also handing every raw event and device list to `trace` (on the reader thread,
/// before the event is mapped). Without a trace nothing extra is collected.
pub fn spawn_traced(display: Option<&str>, callback: impl Fn(Option<Sample>) + Send + 'static, trace: Option<Trace>) -> Result<JoinHandle<()>, Error> {
    let (conn, screen) = RustConnection::connect(display).map_err(|e| Error::Unsupported(format!("cannot connect to the X server: {e}")))?;
    let root = conn.setup().roots.get(screen).map(|s| s.root).ok_or_else(|| Error::Platform(format!("X screen {screen} not found")))?;
    // Raw events for everyone (not just the grabbing client) need XI 2.1; ask for 2.2.
    let version =
        conn.xinput_xi_query_version(2, 2).map_err(|e| Error::Platform(e.to_string()))?.reply().map_err(|e| Error::Unsupported(format!("no XInput2: {e}")))?;
    if (version.major_version, version.minor_version) < (2, 1) {
        return Err(Error::Unsupported(format!("XInput {}.{} (raw events need 2.1)", version.major_version, version.minor_version)));
    }
    let mask = XIEventMask::RAW_MOTION | XIEventMask::RAW_BUTTON_PRESS | XIEventMask::RAW_BUTTON_RELEASE | XIEventMask::HIERARCHY | XIEventMask::DEVICE_CHANGED;
    conn.xinput_xi_select_events(root, &[xinput::EventMask { deviceid: ALL_DEVICES, mask: vec![mask] }])
        .map_err(|e| Error::Platform(e.to_string()))?
        .check()
        .map_err(|e| Error::Platform(format!("XISelectEvents: {e}")))?;
    let mut state = State::default();
    set_devices(&conn, &mut state, trace.as_ref());
    std::thread::Builder::new()
        .name("photocraft-tablet-x11".into())
        .spawn(move || run(&conn, &mut state, &callback, trace.as_ref()))
        .map_err(|e| Error::Platform(format!("cannot start the tablet thread: {e}")))
}

fn run(conn: &RustConnection, state: &mut State, callback: &dyn Fn(Option<Sample>), trace: Option<&Trace>) {
    // A connection error (the display went away) ends the loop; malformed events are skipped by
    // x11rb's parser and by the bounds in `xi`.
    while let Ok(event) = conn.wait_for_event() {
        let raw = match event {
            Event::XinputRawMotion(e) => Some((RawKind::Motion, e)),
            Event::XinputRawButtonPress(e) => Some((RawKind::Press, e)),
            Event::XinputRawButtonRelease(e) => Some((RawKind::Release, e)),
            Event::XinputHierarchy(_) | Event::XinputDeviceChanged(_) => {
                set_devices(conn, state, trace);
                None
            }
            _ => None,
        };
        let Some((kind, e)) = raw else { continue };
        let values: Vec<f64> = e.axisvalues.iter().map(|v| fp3232(v.integral, v.frac)).collect();
        if let Some(t) = trace {
            let valuators = valuators(&e.valuator_mask, &values).collect();
            let button = if kind == RawKind::Motion { 0 } else { e.detail };
            send_trace(t, &Raw::Event { time_ms: e.time, device: e.deviceid, kind, button, valuators });
        }
        if let Update::Set(sample) = state.handle(e.deviceid, valuators(&e.valuator_mask, &values)) {
            deliver(callback, sample);
        }
    }
}

/// Re-read the device list into `state` (and the trace).
fn set_devices(conn: &RustConnection, state: &mut State, trace: Option<&Trace>) {
    let devices = query_devices(conn);
    state.set_devices(devices.iter().map(|d| Device::new(d.id, &d.name, d.valuators.iter().cloned())));
    if let Some(t) = trace {
        send_trace(t, &Raw::Devices(devices));
    }
}

/// Run the trace callback without letting a panic end the reader thread.
fn send_trace(trace: &Trace, raw: &Raw) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| trace(raw)));
}

/// The slave pointer devices with all their valuators (empty on any error).
fn query_devices(conn: &RustConnection) -> Vec<RawDevice> {
    let Ok(reply) = conn.xinput_xi_query_device(ALL_DEVICES).map_err(drop).and_then(|c| c.reply().map_err(drop)) else { return Vec::new() };
    let mut out = Vec::new();
    for info in reply.infos.iter().take(crate::xi::MAX_DEVICES) {
        if info.type_ != DeviceType::SLAVE_POINTER && info.type_ != DeviceType::FLOATING_SLAVE {
            continue;
        }
        let name = String::from_utf8_lossy(&info.name);
        let valuators = info.classes.iter().take(crate::xi::MAX_AXES).filter_map(|c| match &c.data {
            DeviceClassData::Valuator(v) => {
                let label = atom_name(conn, v.label)?;
                Some((v.number, label, fp3232(v.min.integral, v.min.frac), fp3232(v.max.integral, v.max.frac)))
            }
            _ => None,
        });
        out.push(RawDevice { id: info.deviceid, name: name.into_owned(), valuators: valuators.collect() });
    }
    out
}

fn atom_name(conn: &RustConnection, atom: u32) -> Option<String> {
    if atom == 0 {
        return None;
    }
    let reply = conn.get_atom_name(atom).ok()?.reply().ok()?;
    Some(String::from_utf8_lossy(&reply.name).into_owned())
}
