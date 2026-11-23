use crate::config::Calibration;
use crate::edge::{Axis, Geometry, Settings};
use crate::state::{Event, Frame, Tracker};
use evdev::{raw_stream::RawDevice, AbsoluteAxisCode as Abs, KeyCode};
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub fn access_error(path: &Path, error: io::Error) -> String {
    if error.kind() == io::ErrorKind::PermissionDenied {
        format!("{}: permission denied; use sudo e-motional-lib --setup for calibration, or start the installed system service; do not change device permissions or input-group memberships: {error}", path.display())
    } else {
        format!("{}: {error}", path.display())
    }
}

pub fn enumerate() -> io::Result<Vec<udev::Device>> {
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("input")?;
    enumerator.match_property("ID_INPUT_TOUCHPAD", "1")?;
    Ok(enumerator.scan_devices()?.collect())
}

pub fn monitor() -> io::Result<udev::MonitorSocket> {
    udev::MonitorBuilder::new()?
        .match_subsystem("input")?
        .listen()
}

// EVIOCGMTSLOTS reads persistent per-slot values; evdev suppresses unchanged
// coordinates even across a lift/new tracking ID. The bounded stack buffer
// covers the same slot limit accepted during discovery.
fn slot_values(raw: &RawDevice, code: Abs) -> io::Result<[i32; 1025]> {
    let mut values = [0_i32; 1025];
    values[0] = code.0 as i32;
    let request = libc::_IOR::<[i32; 1025]>(b'E' as u32, 0x0a);
    if unsafe { libc::ioctl(raw.as_raw_fd(), request, values.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(values)
}

fn quiet(raw: &RawDevice, slots: usize) -> io::Result<bool> {
    if slots > 0 {
        return Ok(slot_values(raw, Abs::ABS_MT_TRACKING_ID)?[1..=slots]
            .iter()
            .all(|id| *id < 0));
    }
    let keys = raw.get_key_state()?;
    Ok(![
        KeyCode::BTN_TOUCH,
        KeyCode::BTN_TOOL_FINGER,
        KeyCode::BTN_TOOL_DOUBLETAP,
        KeyCode::BTN_TOOL_TRIPLETAP,
        KeyCode::BTN_TOOL_QUADTAP,
        KeyCode::BTN_TOOL_QUINTTAP,
    ]
    .iter()
    .any(|key| keys.contains(*key)))
}

fn bootstrap(raw: &RawDevice, mt: bool, slots: usize) -> io::Result<(Tracker, bool)> {
    let mut tracker = Tracker::new(mt, slots);
    let mut send = |kind, code, value| {
        tracker.event(Event { kind, code, value });
    };
    let axes = raw.get_abs_state()?;
    if slots > 0 {
        let x = slot_values(raw, Abs::ABS_MT_POSITION_X)?;
        let y = slot_values(raw, Abs::ABS_MT_POSITION_Y)?;
        let tools = if raw
            .supported_absolute_axes()
            .is_some_and(|a| a.contains(Abs::ABS_MT_TOOL_TYPE))
        {
            Some(slot_values(raw, Abs::ABS_MT_TOOL_TYPE)?)
        } else {
            None
        };
        for slot in 0..slots {
            send(3, Abs::ABS_MT_SLOT.0, slot as i32);
            send(3, Abs::ABS_MT_POSITION_X.0, x[slot + 1]);
            send(3, Abs::ABS_MT_POSITION_Y.0, y[slot + 1]);
            if let Some(tools) = &tools {
                send(3, Abs::ABS_MT_TOOL_TYPE.0, tools[slot + 1]);
            }
        }
        send(
            3,
            Abs::ABS_MT_SLOT.0,
            axes[Abs::ABS_MT_SLOT.0 as usize].value,
        );
    } else if !mt {
        send(3, Abs::ABS_X.0, axes[Abs::ABS_X.0 as usize].value);
        send(3, Abs::ABS_Y.0, axes[Abs::ABS_Y.0 as usize].value);
    }
    send(
        1,
        KeyCode::BTN_LEFT.0,
        i32::from(raw.get_key_state()?.contains(KeyCode::BTN_LEFT)),
    );
    // Pre-existing contacts have unknown origins. Wait for full lift instead of
    // treating a startup/drop snapshot as a new interior-started finger.
    Ok((tracker, !quiet(raw, slots)?))
}

pub fn calibration_id(device: &udev::Device, name: &str) -> String {
    let syspath = device.syspath().to_string_lossy();
    // inputN/eventN numbering is transient. Prefer udev's persistent port path.
    let location = device
        .property_value("ID_PATH")
        .and_then(|value| value.to_str())
        .unwrap_or_else(|| {
            syspath
                .rsplit_once("/input/input")
                .map_or(syspath.as_ref(), |(parent, _)| parent)
        });
    format!("{location}|{name}")
}

pub fn calibrated_axes(id: &str, reported: [Axis; 2], calibrations: &[Calibration]) -> [Axis; 2] {
    match calibrations.iter().find(|calibration| calibration.id == id) {
        Some(calibration) => [
            Axis {
                min: calibration.min_x,
                max: calibration.max_x,
                ..reported[0]
            },
            Axis {
                min: calibration.min_y,
                max: calibration.max_y,
                ..reported[1]
            },
        ],
        None => reported,
    }
}

pub struct Touchpad {
    pub path: PathBuf,
    pub syspath: PathBuf,
    pub id: String,
    pub axes: [Axis; 2],
    pub raw: RawDevice,
    pub tracker: Tracker,
    multitouch: bool,
    slots: usize,
    quarantined: bool,
}

impl Touchpad {
    pub fn open_configured(
        device: &udev::Device,
        settings: Settings,
        calibrations: &[Calibration],
    ) -> Result<Option<Self>, String> {
        if device.property_value("ID_INPUT_TOUCHPAD") != Some(std::ffi::OsStr::new("1"))
            || device.property_value("ID_INPUT_KEYBOARD") == Some(std::ffi::OsStr::new("1"))
            || device.property_value("ID_SEAT").is_some_and(|seat| seat != "seat0")
        {
            return Ok(None);
        }
        let Some(parent) = device.parent() else {
            return Ok(None);
        };
        let Some(name) = parent.attribute_value("name") else {
            return Ok(None);
        };
        let id = calibration_id(device, name.to_str().unwrap_or("unnamed"));
        if !calibrations.iter().any(|calibration| calibration.id == id) {
            return Ok(None);
        }
        let pad = Self::open(device, settings, false, calibrations)?;
        if pad.as_ref().is_some_and(|pad| pad.id != id) {
            return Err("touchpad identity changed during open; refusing device".into());
        }
        Ok(pad)
    }

    pub fn open(
        device: &udev::Device,
        _settings: Settings,
        list: bool,
        calibrations: &[Calibration],
    ) -> Result<Option<Self>, String> {
        if device.property_value("ID_INPUT_TOUCHPAD") != Some(std::ffi::OsStr::new("1")) {
            log::trace!(
                "{}: skipped: ID_INPUT_TOUCHPAD is not 1",
                device.syspath().display()
            );
            return Ok(None);
        }
        if device.property_value("ID_INPUT_KEYBOARD") == Some(std::ffi::OsStr::new("1")) {
            log::warn!("{}: refusing keyboard-tagged touchpad", device.syspath().display());
            return Ok(None);
        }
        if device.property_value("ID_SEAT").is_some_and(|seat| seat != "seat0") {
            log::debug!("{}: skipping touchpad outside seat0", device.syspath().display());
            return Ok(None);
        }
        let Some(path) = device.devnode().filter(|p| {
            p.parent() == Some(Path::new("/dev/input"))
                && p.file_name().is_some_and(|name| {
                    let name = name.as_encoded_bytes();
                    name.strip_prefix(b"event")
                        .is_some_and(|suffix| !suffix.is_empty() && suffix.iter().all(u8::is_ascii_digit))
                })
        }) else {
            log::trace!(
                "{}: skipped: no evdev event node",
                device.syspath().display()
            );
            return Ok(None);
        };
        log::debug!("{}: opening touchpad O_RDONLY|O_NONBLOCK", path.display());
        // RawDevice::open tries O_RDWR first; use our own strictly O_RDONLY descriptor.
        let fd = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => file,
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    || e.raw_os_error() == Some(libc::ENODEV) =>
            {
                log::debug!("{}: skipped: device vanished: {e}", path.display());
                return Ok(None);
            }
            Err(e) => return Err(access_error(path, e)),
        };
        let metadata = fd.metadata().map_err(|error| access_error(path, error))?;
        if !metadata.file_type().is_char_device() || device.devnum() != Some(metadata.rdev()) {
            return Err(format!("{}: device identity mismatch; refusing input", path.display()));
        }
        let raw = RawDevice::from_fd(fd.into()).map_err(|e| access_error(path, e))?;
        let Some(keys) = raw.supported_keys() else {
            log::debug!("{}: skipped: no key capabilities", path.display());
            return Ok(None);
        };
        if keys.iter().any(|key| key.0 > 0 && key.0 < 0x100) {
            log::warn!("{}: refusing keyboard-capable event node", path.display());
            return Ok(None);
        }
        if !keys.contains(KeyCode::BTN_LEFT) && !keys.contains(KeyCode::BTN_TOOL_FINGER) {
            log::debug!(
                "{}: skipped: neither BTN_LEFT nor BTN_TOOL_FINGER",
                path.display()
            );
            return Ok(None);
        }
        let Some(axes) = raw.supported_absolute_axes() else {
            log::debug!("{}: skipped: no absolute axes", path.display());
            return Ok(None);
        };
        let mt = axes.contains(Abs::ABS_MT_POSITION_X) && axes.contains(Abs::ABS_MT_POSITION_Y);
        let (x, y) = if mt {
            (Abs::ABS_MT_POSITION_X, Abs::ABS_MT_POSITION_Y)
        } else if axes.contains(Abs::ABS_X) && axes.contains(Abs::ABS_Y) {
            (Abs::ABS_X, Abs::ABS_Y)
        } else {
            log::debug!(
                "{}: skipped: neither MT X/Y nor ABS_X/Y axis pair",
                path.display()
            );
            return Ok(None);
        };
        let info = raw.get_abs_state().map_err(|e| access_error(path, e))?;
        let axis = |code: Abs| {
            let a = info[code.0 as usize];
            Axis {
                min: a.minimum as f64,
                max: a.maximum as f64,
                resolution: a.resolution as f64,
            }
        };
        log::debug!(
            "{}: axis_x={:?} axis_y={:?} BTN_LEFT={} BTN_TOOL_FINGER={}",
            path.display(),
            axis(x),
            axis(y),
            keys.contains(KeyCode::BTN_LEFT),
            keys.contains(KeyCode::BTN_TOOL_FINGER)
        );
        let id = calibration_id(device, raw.name().unwrap_or("unnamed"));
        let reported_axes = [axis(x), axis(y)];
        let effective_axes = calibrated_axes(&id, reported_axes, calibrations);
        log::debug!("{}: calibration_id={id:?} reported_axes={reported_axes:?} effective_axes={effective_axes:?}", path.display());
        if calibrations.iter().any(|calibration| calibration.id == id) {
            log::info!(
                "{}: applying calibrated X=[{}, {}] Y=[{}, {}]",
                path.display(),
                effective_axes[0].min,
                effective_axes[0].max,
                effective_axes[1].min,
                effective_axes[1].max
            );
        }
        let geometry = match Geometry::new(effective_axes[0], effective_axes[1]) {
            Ok(g) => g,
            Err(e) => {
                log::warn!("{}: skipping invalid axes: {e}", path.display());
                return Ok(None);
            }
        };
        let slot_count = if mt && axes.contains(Abs::ABS_MT_SLOT) {
            let slot = info[Abs::ABS_MT_SLOT.0 as usize];
            if slot.minimum != 0 || !(0..1024).contains(&slot.maximum) {
                log::warn!("{}: skipping invalid MT slot range", path.display());
                return Ok(None);
            }
            slot.maximum as usize + 1
        } else {
            0
        };
        let dimensions = geometry.dimensions_mm();
        let sizing = if geometry.uses_reported_resolution() {
            "reported mm"
        } else {
            "ratio fallback"
        };
        if list {
            println!(
                "{}: {}\n  {:.1} x {:.1} mm ({sizing}); MT={mt}, slots={slot_count}",
                path.display(),
                raw.name().unwrap_or("unnamed"),
                dimensions[0],
                dimensions[1]
            );
            println!("  calibration id: {id}\n  effective axes: {effective_axes:?}");
        } else {
            log::info!(
                "observing {} ({})",
                path.display(),
                raw.name().unwrap_or("unnamed")
            );
            log::debug!(
                "{:.1} x {:.1} mm ({sizing}); MT={mt}, slots={slot_count}",
                dimensions[0],
                dimensions[1]
            );
        }
        let (tracker, quarantined) =
            bootstrap(&raw, mt, slot_count).map_err(|e| access_error(path, e))?;
        log::debug!("{}: initialized fd={} quarantined={quarantined} (pre-existing contacts require full lift)", path.display(), raw.as_raw_fd());
        Ok(Some(Self {
            path: path.to_owned(),
            syspath: device.syspath().to_owned(),
            id,
            axes: reported_axes,
            raw,
            tracker,
            multitouch: mt,
            slots: slot_count,
            quarantined,
        }))
    }

    pub fn fd(&self) -> RawFd {
        self.raw.as_raw_fd()
    }

    /// Session transitions require a full lift before any new motion can activate.
    pub fn quarantine(&mut self) {
        self.quarantined = true;
        self.tracker.reset();
    }

    // Forward every committed frame; only the worker calculates motion.
    pub fn drain_frames(&mut self, mut frame_ready: impl FnMut(&Frame)) -> io::Result<bool> {
        let stopped = Frame::default();
        loop {
            let events = match self.raw.fetch_events() {
                Ok(events) => events,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.raw_os_error() == Some(libc::ENODEV) => return Ok(false),
                Err(error) => return Err(error),
            };
            let mut received = false;
            for event in events {
                received = true;
                log::trace!("{}: evdev {event:?}", self.path.display());
                if event.event_type().0 == 0 && event.code() == 3 {
                    self.quarantined = true;
                    frame_ready(&stopped);
                    log::warn!("{}: input overflow; waiting for full lift", self.path.display());
                }
                if let Some(frame) = self.tracker.event(Event {
                    kind: event.event_type().0,
                    code: event.code(),
                    value: event.value(),
                }) {
                    frame_ready(if self.quarantined { &stopped } else { frame });
                }
            }
            if !received {
                return Ok(false);
            }
        }
        if self.quarantined && quiet(&self.raw, self.slots)? {
            (self.tracker, self.quarantined) = bootstrap(&self.raw, self.multitouch, self.slots)?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Contact, Frame};
    use crate::edge::Motion;
    use std::time::{Duration, Instant};

    #[test]
    fn only_matching_calibration_changes_trigger_coordinates() {
        let reported = [
            Axis {
                min: 0.0,
                max: 4000.0,
                resolution: 10.0,
            },
            Axis {
                min: 0.0,
                max: 2000.0,
                resolution: 10.0,
            },
        ];
        let calibration = Calibration {
            id: "pad-a".into(),
            min_x: 100.0,
            max_x: 2100.0,
            min_y: 200.0,
            max_y: 1200.0,
        };
        let settings = Settings {
            edge_percent: [10.0, 2.0],
            dwell: Duration::ZERO,
            ..Settings::default()
        };
        let frame = Frame {
            button_left: true,
            contacts: vec![Contact {
                id: 1,
                origin: Some([1100.0, 700.0]),
                position: Some([1950.0, 700.0]),
            }],
        };
        let now = Instant::now();
        for (id, should_move) in [("pad-a", true), ("pad-b", false)] {
            let axes = calibrated_axes(id, reported, std::slice::from_ref(&calibration));
            let mut motion = Motion::new(Geometry::new(axes[0], axes[1]).unwrap(), settings);
            motion.update(&frame, now);
            assert_eq!(motion.active(now), should_move);
            assert_eq!(motion.velocity(now)[0] > 0.0, should_move);
            assert_eq!(motion.velocity(now)[1], 0.0);
        }
    }
}
