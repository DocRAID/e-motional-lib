use crate::config::{self, Calibration, Config};
use crate::device::{self, Touchpad};
use crate::state::{Event, Frame};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Bounds {
    min: [f64; 2],
    max: [f64; 2],
}

#[derive(Default)]
struct Samples {
    observed: Option<Bounds>,
    frames: u64,
}

impl Samples {
    fn observe(&mut self, frame: &Frame) -> bool {
        let [contact] = frame.contacts.as_slice() else {
            return false;
        };
        let Some(position) = contact.position else {
            return false;
        };
        if !position.iter().all(|value| value.is_finite()) {
            return false;
        }
        match &mut self.observed {
            Some(bounds) => {
                for (axis, value) in position.into_iter().enumerate() {
                    bounds.min[axis] = bounds.min[axis].min(value);
                    bounds.max[axis] = bounds.max[axis].max(value);
                }
            }
            None => {
                self.observed = Some(Bounds {
                    min: position,
                    max: position,
                });
            }
        }
        self.frames = self.frames.saturating_add(1);
        true
    }

    fn complete(&self) -> Option<Bounds> {
        let bounds = self.observed?;
        (0..2)
            .all(|axis| {
                let span = bounds.max[axis] - bounds.min[axis];
                span.is_finite() && span > 0.0
            })
            .then_some(bounds)
    }

    fn print(&self) {
        if let Some(bounds) = self.observed {
            println!(
                "Observed raw ranges: X {}..{}, Y {}..{} ({} single-contact frames)",
                bounds.min[0], bounds.max[0], bounds.min[1], bounds.max[1], self.frames
            );
        }
    }
}

// Read one byte at a time so no buffered stdin reader hides a complete line
// from poll. Terminal input remains canonical; no terminal settings are changed.
enum Input {
    Pending,
    Line(String),
    Eof,
}

#[derive(Default)]
struct Lines {
    pending: Vec<u8>,
}

impl Lines {
    fn ready(&mut self) -> Result<Input, String> {
        let mut byte = 0_u8;
        loop {
            let count = unsafe { libc::read(libc::STDIN_FILENO, (&mut byte as *mut u8).cast(), 1) };
            if count == 0 {
                return Ok(Input::Eof);
            }
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(format!("reading setup input: {error}"));
            }
            break;
        }
        if byte != b'\n' {
            self.pending.push(byte);
            return Ok(Input::Pending);
        }
        let line = String::from_utf8(std::mem::take(&mut self.pending))
            .map_err(|_| "setup input must be valid UTF-8".to_string())?;
        Ok(Input::Line(line))
    }

    fn prompt(&mut self, text: &str) -> Result<Option<String>, String> {
        print!("{text}");
        io::stdout()
            .flush()
            .map_err(|error| format!("writing setup prompt: {error}"))?;
        loop {
            let mut descriptors = [stdin_pollfd()];
            poll(&mut descriptors)?;
            check_stdin(descriptors[0].revents)?;
            match self.ready()? {
                Input::Pending => {}
                Input::Line(line) if line.trim().eq_ignore_ascii_case("q") => return Ok(None),
                Input::Line(line) => return Ok(Some(line)),
                Input::Eof => return Ok(None),
            }
        }
    }
}

fn stdin_pollfd() -> libc::pollfd {
    libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    }
}

fn poll(descriptors: &mut [libc::pollfd]) -> Result<(), String> {
    loop {
        let result = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                -1,
            )
        };
        if result >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(format!("polling setup input: {error}"));
        }
    }
}

fn check_stdin(events: libc::c_short) -> Result<(), String> {
    if events & (libc::POLLERR | libc::POLLNVAL) != 0 {
        Err("setup stdin is unavailable; use an interactive terminal".into())
    } else {
        Ok(())
    }
}

enum Capture {
    Complete(Bounds),
    Cancelled,
    Dropped,
}

fn capture(pad: &mut Touchpad, lines: &mut Lines) -> Result<Capture, String> {
    println!("Sweep ONE finger around all four edges and corners, reaching the full usable area.");
    println!("No click is needed. Lift and repeat if needed; avoid extra fingers and palms.");
    println!("Press Enter when finished, or type q then Enter to cancel. Ctrl-C also cancels.");
    println!("Only observed positions are saved; a partial sweep cannot discover untouched edges.");
    let mut samples = Samples::default();
    let mut last_progress: Option<Instant> = None;
    loop {
        let mut descriptors = [
            libc::pollfd {
                fd: pad.fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            stdin_pollfd(),
        ];
        poll(&mut descriptors)?;
        let device_events = descriptors[0].revents;
        if device_events & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(format!(
                "{}: touchpad disconnected or became unavailable; configuration was not saved",
                pad.path.display()
            ));
        }
        if device_events & libc::POLLIN != 0 {
            loop {
                let events = match pad.raw.fetch_events() {
                    Ok(events) => events,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        return Err(format!(
                            "{}: reading calibration events: {error}",
                            pad.path.display()
                        ));
                    }
                };
                let mut received = false;
                for event in events {
                    received = true;
                    let event = Event {
                        kind: event.event_type().0,
                        code: event.code(),
                        value: event.value(),
                    };
                    if event.kind == 0 && event.code == 3 {
                        // Reopen after a full lift rather than reusing lost slot state.
                        return Ok(Capture::Dropped);
                    }
                    if let Some(frame) = pad.tracker.event(event) {
                        if samples.observe(frame)
                            && last_progress
                                .is_none_or(|last| last.elapsed() >= Duration::from_millis(250))
                        {
                            samples.print();
                            last_progress = Some(Instant::now());
                        }
                    }
                }
                if !received {
                    return Err(format!(
                        "{}: touchpad event stream ended",
                        pad.path.display()
                    ));
                }
            }
        }
        let stdin_events = descriptors[1].revents;
        check_stdin(stdin_events)?;
        if stdin_events & (libc::POLLIN | libc::POLLHUP) != 0 {
            match lines.ready()? {
                Input::Pending => {}
                Input::Eof => return Ok(Capture::Cancelled),
                Input::Line(line) if line.trim().eq_ignore_ascii_case("q") => {
                    return Ok(Capture::Cancelled);
                }
                Input::Line(line) if line.trim().is_empty() => {
                    samples.print();
                    if let Some(bounds) = samples.complete() {
                        return Ok(Capture::Complete(bounds));
                    }
                    println!(
                        "Not enough motion: move one finger across BOTH X and Y, then press Enter."
                    );
                }
                Input::Line(_) => {
                    println!("Press Enter to finish, or type q then Enter to cancel.")
                }
            }
        }
    }
}

fn measure(config: &Config, lines: &mut Lines) -> Result<Option<Calibration>, String> {
    let devices =
        device::enumerate().map_err(|error| format!("enumerating touchpads for setup: {error}"))?;
    let mut pads = Vec::new();
    for device in devices {
        // Never apply an old calibration to the measurement being collected.
        if let Some(pad) = Touchpad::open(&device, config.settings(), false, &[])? {
            pads.push((device, pad));
        }
    }
    if pads.is_empty() {
        return Err("no eligible readable touchpads found for setup".into());
    }
    println!("Available touchpads:");
    for (index, (_, pad)) in pads.iter().enumerate() {
        println!(
            "  {}. {} ({})",
            index + 1,
            pad.raw.name().unwrap_or("Unnamed touchpad"),
            pad.path.display()
        );
    }
    let index = loop {
        let Some(line) = lines.prompt("Select touchpad [1], or q to cancel: ")? else {
            return Ok(None);
        };
        if line.trim().is_empty() {
            break 0;
        }
        match line.trim().parse::<usize>() {
            Ok(index) if (1..=pads.len()).contains(&index) => break index - 1,
            _ => println!("Enter a device number from 1 to {}.", pads.len()),
        }
    };
    let (device, mut pad) = pads.swap_remove(index);
    drop(pads);
    println!(
        "Calibrating {} ({})",
        pad.raw.name().unwrap_or("Unnamed touchpad"),
        pad.path.display()
    );
    println!(
        "Device-reported raw ranges: X {}..{}, Y {}..{}",
        pad.axes[0].min, pad.axes[0].max, pad.axes[1].min, pad.axes[1].max
    );
    loop {
        match capture(&mut pad, lines)? {
            Capture::Complete(bounds) => {
                return Ok(Some(Calibration {
                    id: pad.id,
                    min_x: bounds.min[0],
                    max_x: bounds.max[0],
                    min_y: bounds.min[1],
                    max_y: bounds.max[1],
                }));
            }
            Capture::Cancelled => return Ok(None),
            Capture::Dropped => {
                println!("Input overflow (SYN_DROPPED): all samples were discarded.");
                println!("Lift ALL fingers. A new complete edge-and-corner sweep is required.");
                loop {
                    let Some(line) = lines
                        .prompt("With every finger lifted, press Enter to restart (q cancels): ")?
                    else {
                        return Ok(None);
                    };
                    if line.trim().is_empty() {
                        break;
                    }
                }
                pad = Touchpad::open(&device, config.settings(), false, &[])?.ok_or_else(|| {
                    "selected touchpad is no longer available for setup".to_string()
                })?;
            }
        }
    }
}

fn percentage(lines: &mut Lines, axis: &str, default: f64) -> Result<Option<f64>, String> {
    loop {
        let Some(line) = lines.prompt(&format!(
            "{axis} edge width in percent of its axis [{default}] (0 < percent < 50; q cancels): "
        ))?
        else {
            return Ok(None);
        };
        if line.trim().is_empty() {
            return Ok(Some(default));
        }
        match line.trim().parse::<f64>() {
            Ok(value) if value.is_finite() && value > 0.0 && value < 50.0 => {
                return Ok(Some(value));
            }
            _ => println!("Enter a finite percentage strictly between 0 and 50."),
        }
    }
}

pub fn run(path: &Path, config: &mut Config) -> Result<(), String> {
    println!("Touchpad edge calibration (read-only; no input grabs or virtual pointer).");
    println!("Configuration file: {}", path.display());
    println!("Nothing is saved until measurement and both edge-width prompts are complete.");
    let mut lines = Lines::default();
    let Some(calibration) = measure(config, &mut lines)? else {
        println!("Setup cancelled; configuration was not saved.");
        return Ok(());
    };
    let Some(horizontal) =
        percentage(&mut lines, "Horizontal (left/right)", config.edge_percent_x)?
    else {
        println!("Setup cancelled; configuration was not saved.");
        return Ok(());
    };
    let Some(vertical) = percentage(&mut lines, "Vertical (top/bottom)", config.edge_percent_y)?
    else {
        println!("Setup cancelled; configuration was not saved.");
        return Ok(());
    };
    let mut updated = config.clone();
    updated.edge_percent_x = horizontal;
    updated.edge_percent_y = vertical;
    let selected = updated
        .devices
        .iter()
        .position(|entry| entry.id == calibration.id);
    let selected = match selected {
        Some(index) => {
            updated.devices[index] = calibration;
            index
        }
        None => {
            updated.devices.push(calibration);
            updated.devices.len() - 1
        }
    };
    config::save(path, &updated)?;
    *config = updated;
    let calibration = &config.devices[selected];
    println!("Saved configuration to {}", path.display());
    println!(
        "Saved raw ranges: X {}..{}, Y {}..{}; horizontal edges {}%, vertical edges {}%.",
        calibration.min_x,
        calibration.max_x,
        calibration.min_y,
        calibration.max_y,
        horizontal,
        vertical
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Contact;

    fn frame(positions: &[Option<[f64; 2]>]) -> Frame {
        Frame {
            button_left: false,
            contacts: positions
                .iter()
                .enumerate()
                .map(|(id, &position)| Contact {
                    id: id as i32,
                    position,
                    // Deliberately unrelated: calibration uses current positions.
                    origin: Some([0.0, 0.0]),
                })
                .collect(),
        }
    }

    #[test]
    fn captures_nonzero_minima_and_maxima_from_current_positions() {
        let mut samples = Samples::default();
        for position in [[120.0, -45.0], [420.0, 85.0], [220.0, -100.0], [90.0, 10.0]] {
            assert!(samples.observe(&frame(&[Some(position)])));
        }
        assert_eq!(
            samples.complete(),
            Some(Bounds {
                min: [90.0, -100.0],
                max: [420.0, 85.0],
            })
        );
    }

    #[test]
    fn ignores_absent_invalid_and_multiple_contacts() {
        let mut samples = Samples::default();
        for positions in [
            vec![],
            vec![None],
            vec![Some([f64::NAN, 3.0])],
            vec![Some([3.0, f64::INFINITY])],
            vec![Some([f64::NEG_INFINITY, 3.0])],
            vec![Some([1.0, 2.0]), Some([5000.0, 6000.0])],
            vec![Some([1.0, 2.0]), None],
        ] {
            assert!(!samples.observe(&frame(&positions)));
        }
        assert_eq!(samples.observed, None);
        samples.observe(&frame(&[Some([10.0, 20.0])]));
        assert!(!samples.observe(&frame(&[Some([f64::NAN, -1000.0])])));
        assert!(!samples.observe(&frame(&[Some([-1000.0, -1000.0]), None])));
        assert_eq!(
            samples.observed,
            Some(Bounds {
                min: [10.0, 20.0],
                max: [10.0, 20.0],
            })
        );
    }

    #[test]
    fn rejects_no_motion_and_requires_positive_span_on_both_axes() {
        let mut samples = Samples::default();
        assert_eq!(samples.complete(), None);
        samples.observe(&frame(&[Some([10.0, 20.0])]));
        samples.observe(&frame(&[Some([10.0, 20.0])]));
        assert_eq!(samples.complete(), None);
        samples.observe(&frame(&[Some([30.0, 20.0])]));
        assert_eq!(samples.complete(), None);
        samples.observe(&frame(&[Some([30.0, 21.0])]));
        assert_eq!(
            samples.complete(),
            Some(Bounds {
                min: [10.0, 20.0],
                max: [30.0, 21.0],
            })
        );
        let mut vertical_only = Samples::default();
        vertical_only.observe(&frame(&[Some([10.0, 20.0])]));
        vertical_only.observe(&frame(&[Some([10.0, 40.0])]));
        assert_eq!(vertical_only.complete(), None);
    }

    #[test]
    fn rejects_unrepresentable_span_and_accepts_small_positive_spans() {
        let mut samples = Samples::default();
        samples.observe(&frame(&[Some([-f64::MAX, 1.0])]));
        samples.observe(&frame(&[Some([f64::MAX, 2.0])]));
        assert_eq!(samples.complete(), None);
        let mut small = Samples::default();
        small.observe(&frame(&[Some([1.0, 1.0])]));
        small.observe(&frame(&[Some([1.0 + f64::EPSILON, 1.0 + f64::EPSILON])]));
        assert!(small.complete().is_some());
    }
}
