//! Private fixed-size protocol: no descriptors, paths, keys, buttons or reconfiguration.
use crate::edge::{Axis, Geometry, Settings};
use crate::state::{Contact, Frame};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

pub const SIZE: usize = 144;
pub const MAX_DEVICES: usize = 32;
pub const MAX_DELTA: i32 = 128;
pub const IO_TIMEOUT: Duration = Duration::from_millis(100);
const MAGIC: &[u8; 4] = b"EML1";

pub enum Message {
    Add { id: u32, axes: [Axis; 2], settings: Settings },
    Frame { id: u32, contact: Option<Contact> },
    Remove(u32),
    Reset,
    Tick(u64),
    Movement { sequence: u64, delta: [i32; 2], next_ms: u32 },
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid broker protocol packet")
}

fn number(bytes: &[u8], at: usize) -> f64 {
    f64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

impl Message {
    pub fn frame(id: u32, frame: &Frame) -> Self {
        let contact = if frame.contacts.len() == 1 {
            let contact = &frame.contacts[0];
            match (contact.position, contact.origin) {
                (Some(position), Some(origin))
                    if position.iter().chain(origin.iter()).all(|v| v.is_finite()) =>
                {
                    Some(contact.clone())
                }
                _ => None,
            }
        } else {
            None
        };
        Self::Frame { id, contact }
    }

    pub fn encode(&self) -> [u8; SIZE] {
        let mut bytes = [0; SIZE];
        bytes[..4].copy_from_slice(MAGIC);
        let mut id = 0_u32;
        let mut sequence = 0_u64;
        match self {
            Self::Add { id: device, axes, settings } => {
                bytes[4] = 1;
                id = *device;
                let values = [axes[0].min, axes[0].max, axes[0].resolution,
                    axes[1].min, axes[1].max, axes[1].resolution,
                    settings.edge_percent[0], settings.edge_percent[1],
                    settings.edge_width_mm.unwrap_or(0.0), settings.max_speed_scale];
                for (index, value) in values.iter().enumerate() {
                    let at = 24 + index * 8;
                    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
                }
                bytes[104..112].copy_from_slice(&(settings.dwell.as_millis() as u64).to_le_bytes());
            }
            Self::Frame { id: device, contact } => {
                bytes[4] = 2;
                id = *device;
                if let Some(contact) = contact {
                    if let (Some(position), Some(origin)) = (contact.position, contact.origin) {
                        bytes[24] = 1;
                        bytes[28..32].copy_from_slice(&contact.id.to_le_bytes());
                        for (index, value) in position.iter().chain(origin.iter()).enumerate() {
                            let at = 32 + index * 8;
                            bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
                        }
                    }
                }
            }
            Self::Remove(device) => { bytes[4] = 3; id = *device; }
            Self::Reset => bytes[4] = 4,
            Self::Tick(value) => { bytes[4] = 5; sequence = *value; }
            Self::Movement { sequence: value, delta, next_ms } => {
                bytes[4] = 6;
                sequence = *value;
                bytes[24..28].copy_from_slice(&delta[0].to_le_bytes());
                bytes[28..32].copy_from_slice(&delta[1].to_le_bytes());
                bytes[32..36].copy_from_slice(&next_ms.to_le_bytes());
            }
        }
        bytes[8..12].copy_from_slice(&id.to_le_bytes());
        bytes[16..24].copy_from_slice(&sequence.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8; SIZE]) -> io::Result<Self> {
        if &bytes[..4] != MAGIC || bytes[5..8] != [0; 3] || bytes[12..16] != [0; 4] {
            return Err(invalid());
        }
        let id = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let sequence = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let used = match bytes[4] { 1 => 112, 2 => 64, 3..=5 => 24, 6 => 36, _ => return Err(invalid()) };
        if bytes[used..].iter().any(|v| *v != 0)
            || (matches!(bytes[4], 1..=3) && (id == 0 || sequence != 0))
            || (matches!(bytes[4], 4..=6) && id != 0)
            || (bytes[4] == 4 && sequence != 0)
            || (matches!(bytes[4], 5..=6) && sequence == 0)
        {
            return Err(invalid());
        }
        match bytes[4] {
            1 => {
                let values: [f64; 10] = std::array::from_fn(|index| number(bytes, 24 + index * 8));
                if values.iter().any(|v| !v.is_finite())
                    || !(0.0 < values[6] && values[6] < 50.0 && 0.0 < values[7] && values[7] < 50.0)
                    || !(0.0..=1_000_000.0).contains(&values[8])
                    || !(0.0 < values[9] && values[9] <= 1_000_000.0)
                {
                    return Err(invalid());
                }
                let axes = [Axis { min: values[0], max: values[1], resolution: values[2] },
                    Axis { min: values[3], max: values[4], resolution: values[5] }];
                Geometry::new(axes[0], axes[1]).map_err(|_| invalid())?;
                let settings = Settings {
                    edge_percent: [values[6], values[7]],
                    edge_width_mm: (values[8] != 0.0).then_some(values[8]),
                    max_speed_scale: values[9],
                    dwell: Duration::from_millis(u64::from_le_bytes(bytes[104..112].try_into().unwrap())),
                };
                Ok(Self::Add { id, axes, settings })
            }
            2 => {
                if bytes[25..28] != [0; 3] || bytes[24] > 1 {
                    return Err(invalid());
                }
                let contact = if bytes[24] == 0 {
                    if bytes[28..64].iter().any(|v| *v != 0) { return Err(invalid()); }
                    None
                } else {
                    let contact_id = i32::from_le_bytes(bytes[28..32].try_into().unwrap());
                    let values: [f64; 4] = std::array::from_fn(|index| number(bytes, 32 + index * 8));
                    if values.iter().any(|v| !v.is_finite()) { return Err(invalid()); }
                    Some(Contact { id: contact_id, position: Some([values[0], values[1]]), origin: Some([values[2], values[3]]) })
                };
                Ok(Self::Frame { id, contact })
            }
            3 => Ok(Self::Remove(id)),
            4 => Ok(Self::Reset),
            5 => Ok(Self::Tick(sequence)),
            6 => {
                let delta = [i32::from_le_bytes(bytes[24..28].try_into().unwrap()),
                    i32::from_le_bytes(bytes[28..32].try_into().unwrap())];
                let next_ms = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
                if delta.iter().any(|v| !(-MAX_DELTA..=MAX_DELTA).contains(v)) || !(8..=1000).contains(&next_ms) { return Err(invalid()); }
                Ok(Self::Movement { sequence, delta, next_ms })
            }
            _ => Err(invalid()),
        }
    }
}

pub fn wait(fd: RawFd, events: i16, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Err(io::Error::new(io::ErrorKind::TimedOut, "broker IPC timeout")); }
        let mut descriptor = libc::pollfd { fd, events, revents: 0 };
        let result = unsafe { libc::poll(&mut descriptor, 1, remaining.as_millis().saturating_add(1).min(i32::MAX as u128) as i32) };
        if result > 0 {
            if descriptor.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "broker IPC disconnected"));
            }
            return Ok(());
        }
        if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error());
        }
    }
}

pub fn send(stream: &mut UnixStream, message: &Message) -> io::Result<()> {
    let bytes = message.encode();
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut offset = 0;
    while offset < SIZE {
        match stream.write(&bytes[offset..]) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "broker IPC disconnected")),
            Ok(count) => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait(stream.as_raw_fd(), libc::POLLOUT, deadline)?,
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline { return Err(io::Error::new(io::ErrorKind::TimedOut, "broker IPC write timeout")); }
    }
    Ok(())
}

pub struct Reader {
    bytes: [u8; SIZE],
    offset: usize,
    started: Option<Instant>,
}

impl Reader {
    pub fn new() -> Self { Self { bytes: [0; SIZE], offset: 0, started: None } }

    pub fn receive(&mut self, stream: &mut UnixStream) -> io::Result<Option<Message>> {
        self.check_timeout()?;
        loop {
            match stream.read(&mut self.bytes[self.offset..]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "broker IPC disconnected")),
                Ok(count) => {
                    self.started.get_or_insert_with(Instant::now);
                    self.offset += count;
                    if self.offset == SIZE {
                        self.offset = 0;
                        self.started = None;
                        return Message::decode(&self.bytes).map(Some);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) => return Err(error),
            }
            self.check_timeout()?;
        }
    }

    pub fn deadline(&self) -> Option<Instant> { self.started.map(|time| time + IO_TIMEOUT) }

    pub fn check_timeout(&self) -> io::Result<()> {
        if self.started.is_some_and(|time| time.elapsed() > IO_TIMEOUT) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "partial broker IPC packet timeout"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_reserved_and_nonfinite_fields() {
        let mut packet = Message::Tick(1).encode();
        packet[5] = 1;
        assert!(Message::decode(&packet).is_err());
        packet = Message::Tick(1).encode();
        packet[143] = 1;
        assert!(Message::decode(&packet).is_err());
        packet = Message::Tick(1).encode();
        packet[4] = 99;
        assert!(Message::decode(&packet).is_err());
        let frame = Frame { contacts: vec![Contact { id: 2, position: Some([1.0, 2.0]), origin: Some([3.0, 4.0]) }], ..Frame::default() };
        packet = Message::frame(1, &frame).encode();
        packet[32..40].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(Message::decode(&packet).is_err());
    }

    #[test]
    fn movement_has_only_bounded_relative_axes_and_nonzero_sequence() {
        assert!(Message::decode(&Message::Movement { sequence: 1, delta: [-128, 128], next_ms: 8 }.encode()).is_ok());
        for delta in [[i32::MIN, 0], [0, 129]] {
            assert!(Message::decode(&Message::Movement { sequence: 1, delta, next_ms: 8 }.encode()).is_err());
        }
        assert!(Message::decode(&Message::Tick(0).encode()).is_err());
        for next_ms in [0, 7, 1001, u32::MAX] {
            assert!(Message::decode(&Message::Movement { sequence: 1, delta: [0, 0], next_ms }.encode()).is_err());
        }
    }

    #[test]
    fn multifinger_or_unknown_origin_is_stop() {
        let mut frame = Frame { contacts: vec![Contact { id: 1, position: Some([1.0; 2]), origin: None }], ..Frame::default() };
        assert!(matches!(Message::frame(1, &frame), Message::Frame { contact: None, .. }));
        frame.contacts[0].origin = Some([2.0; 2]);
        frame.contacts.push(frame.contacts[0].clone());
        assert!(matches!(Message::frame(1, &frame), Message::Frame { contact: None, .. }));
    }

    #[test]
    fn anonymous_type_a_contacts_can_still_activate_edge_motion() {
        let input = Frame {
            contacts: vec![Contact { id: -2, position: Some([99.0, 50.0]), origin: Some([50.0, 50.0]) }],
            ..Frame::default()
        };
        let packet = Message::frame(1, &input).encode();
        let Message::Frame { contact: Some(contact), .. } = Message::decode(&packet).unwrap() else {
            panic!("a known synthetic contact must not become a stop frame");
        };
        let axis = Axis { min: 0.0, max: 100.0, resolution: 1.0 };
        let mut motion = crate::edge::Motion::new(
            Geometry::new(axis, axis).unwrap(),
            Settings { dwell: Duration::ZERO, ..Settings::default() },
        );
        let now = Instant::now();
        motion.update(&Frame { contacts: vec![contact], ..Frame::default() }, now);
        assert!(motion.velocity(now)[0] > 0.0);
        assert_eq!(motion.velocity(now)[1], 0.0);
    }

    #[test]
    fn reader_preserves_partial_packets_without_unbounded_buffering() {
        let (mut left, mut right) = UnixStream::pair().unwrap();
        right.set_nonblocking(true).unwrap();
        let packet = Message::Tick(7).encode();
        let mut reader = Reader::new();
        left.write_all(&packet[..17]).unwrap();
        assert!(reader.receive(&mut right).unwrap().is_none());
        left.write_all(&packet[17..]).unwrap();
        assert!(matches!(reader.receive(&mut right).unwrap(), Some(Message::Tick(7))));
    }
}
