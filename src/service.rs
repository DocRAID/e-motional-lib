use crate::config::Config;
use crate::device::{self, Touchpad};
use crate::edge::{Accumulator, Geometry, Motion, TICK_HZ};
use crate::inject::Injector;
use crate::protocol::{self, Message, Reader, IO_TIMEOUT, MAX_DELTA, MAX_DEVICES};
use crate::session::Gate;
use crate::state::Frame;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_nanos(1_000_000_000 / TICK_HZ);
const SOCKET_PATH: &str = "/run/e-motional-lib/broker.sock";

fn activation(pid: &str, count: &str, names: &str, expected: &str, current_pid: u32) -> bool {
    pid.parse::<u32>() == Ok(current_pid) && count == "1" && names == expected
}

fn inherited(expected: &str, listening: bool) -> Result<RawFd, String> {
    let value = |name| std::env::var(name).unwrap_or_default();
    if !activation(&value("LISTEN_PID"), &value("LISTEN_FDS"), &value("LISTEN_FDNAMES"), expected, std::process::id()) {
        return Err("start e-motional-lib with its installed systemd service; the inherited broker socket is required".into());
    }
    let fd = 3;
    let option = |name| -> Result<libc::c_int, String> {
        let mut value: libc::c_int = 0;
        let mut length = std::mem::size_of_val(&value) as libc::socklen_t;
        if unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, name, (&mut value as *mut libc::c_int).cast(), &mut length) } < 0 {
            return Err(format!("inherited systemd socket: {}", io::Error::last_os_error()));
        }
        Ok(value)
    };
    if option(libc::SO_TYPE)? != libc::SOCK_STREAM || option(libc::SO_DOMAIN)? != libc::AF_UNIX
        || option(libc::SO_ACCEPTCONN)? != i32::from(listening)
    { return Err("invalid inherited systemd Unix stream socket".into()); }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(format!("socket close-on-exec: {}", io::Error::last_os_error()));
    }
    // Activation is consumed once; never expose an inherited endpoint to children.
    for name in ["LISTEN_PID", "LISTEN_FDS", "LISTEN_FDNAMES"] { std::env::remove_var(name); }
    Ok(fd)
}

fn root_peer(stream: &UnixStream) -> Result<(), String> {
    let mut credentials = unsafe { std::mem::zeroed::<libc::ucred>() };
    let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
    if unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut credentials as *mut libc::ucred).cast(), &mut length) } < 0
        || length as usize != std::mem::size_of::<libc::ucred>() || credentials.uid != 0 || credentials.pid <= 0
    { return Err("broker socket peer must be root (opened by the system manager)".into()); }
    Ok(())
}

fn ipc(error: io::Error) -> String { format!("broker IPC: {error}") }
fn send(stream: &mut UnixStream, message: &Message) -> Result<(), String> { protocol::send(stream, message).map_err(ipc) }

struct Pad {
    device: Touchpad,
    id: u32,
    contact: bool,
}

struct Pending {
    sequence: u64,
    revision: u64,
    sent: Instant,
}

fn advance(revision: &mut u64) -> Result<(), String> {
    *revision = revision.checked_add(1).ok_or("broker sequence exhausted")?;
    Ok(())
}

fn add_pad(pads: &mut BTreeMap<RawFd, Pad>, node: &udev::Device, config: &Config,
    stream: &mut UnixStream, next_id: &mut u32, revision: &mut u64) -> Result<(), String>
{
    if pads.values().any(|pad| pad.device.syspath == node.syspath()) { return Ok(()); }
    if let Some(mut device) = Touchpad::open_configured(node, config.settings(), &config.devices)? {
        if pads.len() >= MAX_DEVICES { return Err(format!("at most {MAX_DEVICES} selected touchpads are supported")); }
        let axes = device::calibrated_axes(&device.id, device.axes, &config.devices);
        let id = *next_id;
        *next_id = next_id.checked_add(1).ok_or("touchpad identity counter exhausted")?;
        device.quarantine();
        send(stream, &Message::Add { id, axes, settings: config.settings() })?;
        pads.insert(device.fd(), Pad { device, id, contact: false });
        advance(revision)?;
    }
    Ok(())
}

fn remove_pad(pads: &mut BTreeMap<RawFd, Pad>, descriptor: RawFd,
    stream: &mut UnixStream, revision: &mut u64) -> Result<(), String>
{
    if let Some(pad) = pads.remove(&descriptor) {
        send(stream, &Message::Remove(pad.id))?;
        advance(revision)?;
    }
    Ok(())
}

fn drain(pads: &mut BTreeMap<RawFd, Pad>, stream: &mut UnixStream,
    allowed: bool, revision: &mut u64) -> Result<(), String>
{
    let mut removed = [0; MAX_DEVICES];
    let mut count = 0;
    for (descriptor, pad) in pads.iter_mut() {
        let mut failure = None;
        let alive = pad.device.drain_frames(|frame| {
            if failure.is_some() { return; }
            let message = if allowed { Message::frame(pad.id, frame) }
                else { Message::Frame { id: pad.id, contact: None } };
            pad.contact = matches!(&message, Message::Frame { contact: Some(_), .. });
            if let Err(error) = advance(revision).and_then(|()| send(stream, &message)) { failure = Some(error); }
        }).map_err(|error| device::access_error(&pad.device.path, error))?;
        if let Some(error) = failure { return Err(error); }
        if !alive { removed[count] = *descriptor; count += 1; }
    }
    for descriptor in &removed[..count] { remove_pad(pads, *descriptor, stream, revision)?; }
    Ok(())
}

fn session_update(gate: &mut Gate, notified: bool, pads: &mut BTreeMap<RawFd, Pad>,
    stream: &mut UnixStream, revision: &mut u64) -> Result<(), String>
{
    if gate.refresh(notified)? {
        for pad in pads.values_mut() {
            pad.device.quarantine();
            pad.contact = false;
        }
        advance(revision)?;
        send(stream, &Message::Reset)?;
    }
    Ok(())
}

pub fn broker(config: &Config) -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 { return Err("the device broker requires root".into()); }
    config.validate()?;
    if config.devices.is_empty() { return Err("no selected touchpads; run sudo e-motional-lib --setup first".into()); }
    if config.devices.len() > MAX_DEVICES { return Err(format!("at most {MAX_DEVICES} selected touchpads are supported")); }
    let descriptor = inherited("broker-listener", true)?;
    let listener = unsafe { UnixListener::from_raw_fd(descriptor) };
    if listener.local_addr().map_err(ipc)?.as_pathname() != Some(std::path::Path::new(SOCKET_PATH)) {
        return Err("unexpected broker listening socket path".into());
    }
    for (path, directory) in [("/run/e-motional-lib", true), (SOCKET_PATH, false)] {
        let metadata = fs::symlink_metadata(path).map_err(ipc)?;
        if metadata.uid() != 0 || metadata.gid() != 0 || metadata.mode() & 0o777 != if directory { 0o700 } else { 0o600 }
            || if directory { !metadata.is_dir() } else { !metadata.file_type().is_socket() }
        { return Err(format!("broker endpoint must be root-only: {path}")); }
    }
    listener.set_nonblocking(true).map_err(ipc)?;
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(connection) => break connection,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let mut pollfd = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                let result = unsafe { libc::poll(&mut pollfd, 1, -1) };
                if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted { return Err(ipc(io::Error::last_os_error())); }
            }
            Err(error) => return Err(ipc(error)),
        }
    };
    root_peer(&stream)?;
    stream.set_nonblocking(true).map_err(ipc)?;
    // Only one manager-opened worker connection is used per broker lifetime.
    drop(listener);
    let mut gate = Gate::new()?;
    let mut injector = Injector::new()?;
    let monitor = device::monitor().map_err(|error| format!("udev monitor: {error}"))?;
    let mut pads = BTreeMap::new();
    let mut next_id = 1;
    let mut revision = 1;
    for node in device::enumerate().map_err(|error| format!("udev enumeration: {error}"))? {
        add_pad(&mut pads, &node, config, &mut stream, &mut next_id, &mut revision)?;
    }
    session_update(&mut gate, true, &mut pads, &mut stream, &mut revision)?;
    let mut reader = Reader::new();
    let mut sequence = 0_u64;
    let mut pending: Option<Pending> = None;
    let mut next_tick = Instant::now();
    let mut last_revision = 0;
    let mut last_request = Instant::now() - TICK;
    let mut last_emit = Instant::now() - TICK;
    loop {
        let mut descriptors = [libc::pollfd { fd: -1, events: libc::POLLIN, revents: 0 }; MAX_DEVICES + 3];
        descriptors[0].fd = stream.as_raw_fd();
        descriptors[1].fd = monitor.as_raw_fd();
        descriptors[2].fd = gate.fd();
        for (slot, descriptor) in pads.keys().enumerate() { descriptors[slot + 3].fd = *descriptor; }
        let count = pads.len() + 3;
        let wake = pending.as_ref().map_or(next_tick.max(last_request + TICK).max(last_emit + TICK), |request| request.sent + IO_TIMEOUT);
        let wake = reader.deadline().map_or(wake, |deadline| wake.min(deadline));
        let timeout = wake.saturating_duration_since(Instant::now()).as_millis().saturating_add(1).min(1000) as i32;
        let result = unsafe { libc::poll(descriptors.as_mut_ptr(), count as libc::nfds_t, timeout) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted { continue; }
            return Err(format!("broker poll: {error}"));
        }
        if descriptors[..3].iter().any(|fd| fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0) {
            return Err("broker IPC, udev or logind monitor disconnected; refusing cursor output".into());
        }
        session_update(&mut gate, descriptors[2].revents != 0, &mut pads, &mut stream, &mut revision)?;
        if descriptors[1].revents != 0 {
            for event in monitor.iter() {
                if matches!(event.event_type(), udev::EventType::Remove | udev::EventType::Change) {
                    if let Some(descriptor) = pads.iter().find_map(|(fd, pad)| (pad.device.syspath == event.syspath()).then_some(*fd)) {
                        remove_pad(&mut pads, descriptor, &mut stream, &mut revision)?;
                    }
                }
                if matches!(event.event_type(), udev::EventType::Add | udev::EventType::Change) {
                    add_pad(&mut pads, &event, config, &mut stream, &mut next_id, &mut revision)?;
                }
            }
        }
        // Physical stop frames always precede processing movement, even on timer wakes.
        drain(&mut pads, &mut stream, gate.allowed(), &mut revision)?;
        reader.check_timeout().map_err(ipc)?;
        if let Some(message) = reader.receive(&mut stream).map_err(ipc)? {
            let Message::Movement { sequence: reply, delta, next_ms } = message else { return Err("worker sent a forbidden broker command".into()); };
            let request = pending.take().ok_or("unsolicited worker movement")?;
            if reply != request.sequence { return Err("stale or mismatched worker movement sequence".into()); }
            next_tick = request.sent + Duration::from_millis(u64::from(next_ms));
            // Writes/partial reads may have taken time: drain again before using a reply.
            session_update(&mut gate, false, &mut pads, &mut stream, &mut revision)?;
            drain(&mut pads, &mut stream, gate.allowed(), &mut revision)?;
            let now = Instant::now();
            if may_emit(&request, revision, gate.allowed(), pads.values().any(|pad| pad.contact), now, last_emit) {
                injector.emit(delta).map_err(|error| format!("uinput movement: {error}"))?;
                last_emit = now;
            }
            // More than one reply for one tick is a protocol violation, not a backlog.
            if reader.receive(&mut stream).map_err(ipc)?.is_some() { return Err("worker sent duplicate movement".into()); }
        }
        let now = Instant::now();
        if pending.as_ref().is_some_and(|request| now.duration_since(request.sent) > IO_TIMEOUT) {
            return Err("worker stalled; refusing cursor output".into());
        }
        if revision != last_revision { next_tick = next_tick.min(now); }
        if pending.is_none() && now >= next_tick && now.duration_since(last_request) >= TICK
            && now.duration_since(last_emit) >= TICK {
            advance(&mut sequence)?;
            send(&mut stream, &Message::Tick(sequence))?;
            last_request = Instant::now();
            last_revision = revision;
            next_tick = last_request + Duration::from_secs(1);
            pending = Some(Pending { sequence, revision, sent: last_request });
        }
    }
}

fn may_emit(request: &Pending, revision: u64, active_session: bool, contact: bool,
    now: Instant, last_emit: Instant) -> bool
{
    active_session && contact && request.revision == revision
        && now.saturating_duration_since(request.sent) <= IO_TIMEOUT
        && now.saturating_duration_since(last_emit) >= TICK
}

struct WorkerPad {
    motion: Motion,
    accumulator: Accumulator,
    last_tick: Option<Instant>,
}

impl WorkerPad {
    fn reset(&mut self) {
        self.motion.reset();
        self.accumulator.reset();
        self.last_tick = None;
    }
}

pub fn worker() -> Result<(), String> {
    let descriptor = inherited("broker", false)?;
    let mut stream = unsafe { UnixStream::from_raw_fd(descriptor) };
    root_peer(&stream)?;
    if !(61184..=65519).contains(&unsafe { libc::geteuid() }) {
        return Err("the motion worker requires a systemd DynamicUser UID, not root or a regular user".into());
    }
    stream.set_nonblocking(true).map_err(ipc)?;
    let mut reader = Reader::new();
    let mut pads: BTreeMap<u32, WorkerPad> = BTreeMap::new();
    let mut frame = Frame::default();
    frame.contacts.reserve_exact(1);
    let mut previous_sequence = 0_u64;
    loop {
        let timeout = if previous_sequence == 0 { Duration::from_secs(5) } else { Duration::from_secs(2) };
        let deadline = Instant::now() + timeout;
        let deadline = reader.deadline().map_or(deadline, |partial| deadline.min(partial));
        protocol::wait(stream.as_raw_fd(), libc::POLLIN, deadline).map_err(ipc)?;
        let Some(message) = reader.receive(&mut stream).map_err(ipc)? else {
            reader.check_timeout().map_err(ipc)?;
            continue;
        };
        let now = Instant::now();
        match message {
            Message::Add { id, axes, settings } => {
                if pads.len() >= MAX_DEVICES || pads.contains_key(&id) { return Err("duplicate or excess broker touchpad".into()); }
                let geometry = Geometry::new(axes[0], axes[1])?;
                pads.insert(id, WorkerPad { motion: Motion::new(geometry, settings), accumulator: Accumulator::default(), last_tick: None });
            }
            Message::Frame { id, contact } => {
                let pad = pads.get_mut(&id).ok_or("unknown broker touchpad")?;
                frame.contacts.clear();
                if let Some(contact) = contact { frame.contacts.push(contact); }
                pad.motion.update(&frame, now);
                if !pad.motion.active(now) {
                    pad.last_tick = None;
                    pad.accumulator.reset();
                }
            }
            Message::Remove(id) => {
                if pads.remove(&id).is_none() { return Err("removed unknown broker touchpad".into()); }
            }
            Message::Reset => { for pad in pads.values_mut() { pad.reset(); } }
            Message::Tick(sequence) => {
                if sequence != previous_sequence.checked_add(1).ok_or("worker sequence exhausted")? {
                    return Err("out-of-order broker tick".into());
                }
                previous_sequence = sequence;
                let mut total = [0_i32; 2];
                let mut next_ms = 1000_u32;
                for pad in pads.values_mut() {
                    if pad.motion.active(now) {
                        next_ms = 8;
                        if let Some(previous) = pad.last_tick.replace(now) {
                            // Never replay elapsed motion after scheduling or IPC delays.
                            let dt = now.duration_since(previous).min(TICK * 2);
                            let delta = pad.accumulator.step(pad.motion.velocity(now), dt);
                            for axis in 0..2 { total[axis] = total[axis].saturating_add(delta[axis]); }
                        }
                    } else {
                        pad.last_tick = None;
                        pad.accumulator.reset();
                        if let Some(deadline) = pad.motion.deadline().filter(|time| *time > now) {
                            let delay = deadline.duration_since(now).as_millis().saturating_add(1).clamp(8, 1000) as u32;
                            next_ms = next_ms.min(delay);
                        }
                    }
                }
                for value in &mut total { *value = (*value).clamp(-MAX_DELTA, MAX_DELTA); }
                send(&mut stream, &Message::Movement { sequence, delta: total, next_ms })?;
            }
            Message::Movement { .. } => return Err("broker sent a forbidden worker command".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_requires_exact_role_and_single_current_process_fd() {
        assert!(activation("42", "1", "broker", "broker", 42));
        for (pid, count, name) in [("41", "1", "broker"), ("42", "2", "broker"), ("42", "1", "broker:other"), ("42", "1", "broker-listener")] {
            assert!(!activation(pid, count, name, "broker", 42));
        }
    }

    #[test]
    fn stale_stopped_session_inactive_and_rate_limited_replies_cannot_move() {
        let now = Instant::now();
        let request = Pending { sequence: 1, revision: 7, sent: now };
        assert!(may_emit(&request, 7, true, true, now, now - TICK));
        assert!(!may_emit(&request, 8, true, true, now, now - TICK));
        assert!(!may_emit(&request, 7, false, true, now, now - TICK));
        assert!(!may_emit(&request, 7, true, false, now, now - TICK));
        assert!(!may_emit(&request, 7, true, true, now + IO_TIMEOUT + TICK, now - TICK));
        assert!(!may_emit(&request, 7, true, true, now, now));
    }
}
