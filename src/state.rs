//! Frame-committed evdev contact tracking. Numeric codes are Linux input-event codes.
//!
//! Type A devices without tracking IDs cannot identify fingers after an ambiguous
//! multi-contact frame. Such contacts keep unknown origins until a full lift;
//! guessing an origin could incorrectly make an edge-started touch eligible.

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const SYN_REPORT: u16 = 0;
const SYN_MT_REPORT: u16 = 2;
const SYN_DROPPED: u16 = 3;
const BTN_LEFT: u16 = 0x110;
const BTN_TOOL_FINGER: u16 = 0x145;
const BTN_TOOL_QUINTTAP: u16 = 0x148;
const BTN_TOUCH: u16 = 0x14a;
const BTN_TOOL_DOUBLETAP: u16 = 0x14d;
const BTN_TOOL_TRIPLETAP: u16 = 0x14e;
const BTN_TOOL_QUADTAP: u16 = 0x14f;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TOOL_TYPE: u16 = 0x37;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const MT_TOOL_PALM: i32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Contact {
    pub id: i32,
    pub position: Option<[f64; 2]>,
    pub origin: Option<[f64; 2]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub button_left: bool,
    pub contacts: Vec<Contact>,
}

fn position(axes: [Option<f64>; 2]) -> Option<[f64; 2]> {
    Some([axes[0]?, axes[1]?])
}

#[derive(Default)]
struct Slot {
    id: Option<i32>,
    axes: [Option<f64>; 2],
    origin: Option<[f64; 2]>,
    palm: bool,
}

impl Slot {
    fn event(&mut self, code: u16, value: i32) {
        match code {
            ABS_MT_TRACKING_ID if value < 0 => {
                self.id = None;
                self.origin = None;
            }
            ABS_MT_TRACKING_ID if self.id != Some(value) => {
                self.id = Some(value);
                self.origin = None;
            }
            ABS_MT_POSITION_X | ABS_MT_POSITION_Y => {
                let axis = usize::from(code == ABS_MT_POSITION_Y);
                self.axes[axis] = Some(f64::from(value));
            }
            ABS_MT_TOOL_TYPE => {
                self.palm = value == MT_TOOL_PALM;
            }
            _ => {}
        }
    }

    fn commit(&mut self) -> Option<Contact> {
        let current = position(self.axes);
        if self.id.is_some() && self.origin.is_none() {
            self.origin = current;
        }
        if self.palm {
            return None;
        }
        Some(Contact {
            id: self.id?,
            position: current,
            origin: self.origin,
        })
    }
}

struct TypeB {
    slots: Vec<Slot>,
    selected: Option<usize>,
}

impl TypeB {
    fn event(&mut self, event: Event) {
        if event.code == ABS_MT_SLOT {
            self.selected = usize::try_from(event.value)
                .ok()
                .filter(|&slot| slot < self.slots.len());
        } else if let Some(slot) = self.selected {
            self.slots[slot].event(event.code, event.value);
        }
    }

    fn commit(&mut self, contacts: &mut Vec<Contact>) {
        contacts.extend(self.slots.iter_mut().filter_map(Slot::commit));
    }

    fn reset(&mut self) {
        for slot in &mut self.slots {
            *slot = Slot::default();
        }
        // A dropped stream also loses the kernel's selected-slot state.
        self.selected = None;
    }
}

#[derive(Default)]
struct SingleTouch {
    touch: bool,
    finger: bool,
    multiple: [bool; 4],
    axes: [Option<f64>; 2],
    origin: Option<[f64; 2]>,
    was_active: bool,
    released_touch: bool,
    restarted: bool,
}

impl SingleTouch {
    fn event(&mut self, event: Event) {
        let down = event.value != 0;
        match (event.kind, event.code) {
            (EV_KEY, BTN_TOUCH) => {
                if self.touch && !down {
                    self.released_touch = true;
                }
                if down && self.released_touch {
                    self.restarted = true;
                }
                self.touch = down;
            }
            (EV_KEY, BTN_TOOL_FINGER) => self.finger = down,
            (EV_KEY, BTN_TOOL_DOUBLETAP) => self.multiple[0] = down,
            (EV_KEY, BTN_TOOL_TRIPLETAP) => self.multiple[1] = down,
            (EV_KEY, BTN_TOOL_QUADTAP) => self.multiple[2] = down,
            (EV_KEY, BTN_TOOL_QUINTTAP) => self.multiple[3] = down,
            (EV_ABS, ABS_X | ABS_Y) => {
                let axis = usize::from(event.code == ABS_Y);
                self.axes[axis] = Some(f64::from(event.value));
            }
            _ => {}
        }
    }

    fn commit(&mut self, contacts: &mut Vec<Contact>) {
        let multiple = self.multiple.iter().rposition(|&down| down);
        let count =
            multiple.map_or_else(|| usize::from(self.touch || self.finger), |index| index + 2);
        if count == 0 {
            self.origin = None;
        } else {
            if !self.was_active || self.restarted {
                self.origin = None;
            }
            let current = position(self.axes);
            if self.origin.is_none() {
                self.origin = current;
            }
            contacts.push(Contact {
                id: 0,
                position: current,
                origin: self.origin,
            });
            // Tool-count keys report a count, not coordinates for those fingers.
            for id in 1..count {
                contacts.push(Contact {
                    id: id as i32,
                    position: None,
                    origin: None,
                });
            }
        }
        self.was_active = count != 0;
        self.released_touch = false;
        self.restarted = false;
    }
}

#[derive(Default)]
struct Packet {
    seen: bool,
    id: Option<i32>,
    axes: [Option<f64>; 2],
    palm: Option<bool>,
}

impl Packet {
    fn event(&mut self, event: Event) {
        match event.code {
            ABS_MT_TRACKING_ID => self.id = Some(event.value),
            ABS_MT_POSITION_X => self.axes[0] = Some(f64::from(event.value)),
            ABS_MT_POSITION_Y => self.axes[1] = Some(f64::from(event.value)),
            ABS_MT_TOOL_TYPE => self.palm = Some(event.value == MT_TOOL_PALM),
            _ => return,
        }
        self.seen = true;
    }
}

struct TypeAContact {
    id: i32,
    explicit_id: Option<i32>,
    axes: [Option<f64>; 2],
    origin: Option<[f64; 2]>,
    may_initialize_origin: bool,
    palm: bool,
}

#[derive(Default)]
struct TypeA {
    packet: Packet,
    packets: Vec<Packet>,
    previous: Vec<TypeAContact>,
    spare: Vec<TypeAContact>,
    next_id: i32,
    recovering: bool,
}

impl TypeA {
    fn finish_packet(&mut self) {
        let packet = std::mem::take(&mut self.packet);
        if packet.seen && !packet.id.is_some_and(|id| id < 0) {
            self.packets.push(packet);
        }
    }

    fn synthetic_id(&mut self) -> i32 {
        // Linux tracking IDs are nonnegative; synthetic IDs cannot collide.
        if self.next_id >= -1 || self.next_id == i32::MIN {
            self.next_id = -2;
        }
        let id = self.next_id;
        self.next_id -= 1;
        id
    }

    fn commit(&mut self, contacts: &mut Vec<Contact>) {
        self.finish_packet();
        let mut packets = std::mem::take(&mut self.packets);
        let anonymous_count = packets.iter().filter(|p| p.id.is_none()).count();
        let old_anonymous_count = self
            .previous
            .iter()
            .filter(|p| p.explicit_id.is_none())
            .count();
        let anonymous_continuation = anonymous_count == 1 && old_anonymous_count == 1;
        let mut current = std::mem::take(&mut self.spare);
        current.clear();
        for packet in &packets {
            let old = match packet.id {
                Some(id) => {
                    // Duplicate IDs are ambiguous, not evidence of continuity.
                    let unique = packets.iter().filter(|p| p.id == Some(id)).count() == 1;
                    if unique {
                        self.previous.iter().find(|p| p.explicit_id == Some(id))
                    } else {
                        None
                    }
                }
                None if anonymous_continuation => {
                    self.previous.iter().find(|p| p.explicit_id.is_none())
                }
                None => None,
            };
            let (old_id, old_axes, old_origin, old_may_initialize, old_palm) = old
                .map(|p| {
                    (
                        Some(p.id),
                        p.axes,
                        p.origin,
                        p.may_initialize_origin,
                        p.palm,
                    )
                })
                .unwrap_or((None, [None; 2], None, false, false));
            let duplicate_id = packet
                .id
                .is_some_and(|id| packets.iter().filter(|p| p.id == Some(id)).count() != 1);
            let may_initialize_origin = if old_id.is_some() {
                old_may_initialize
            } else {
                !self.recovering
                    && !duplicate_id
                    && (packet.id.is_some() || (anonymous_count == 1 && old_anonymous_count == 0))
            };
            let axes = [
                packet.axes[0].or(old_axes[0]),
                packet.axes[1].or(old_axes[1]),
            ];
            let origin = if may_initialize_origin {
                old_origin.or_else(|| position(axes))
            } else {
                None
            };
            let id = packet.id.or(old_id).unwrap_or_else(|| self.synthetic_id());
            current.push(TypeAContact {
                id,
                explicit_id: packet.id,
                axes,
                origin,
                may_initialize_origin,
                palm: packet.palm.unwrap_or(old_palm),
            });
        }
        std::mem::swap(&mut self.previous, &mut current);
        self.spare = current;
        packets.clear();
        self.packets = packets;
        self.recovering = false;
        contacts.extend(
            self.previous
                .iter()
                .filter(|contact| !contact.palm)
                .map(|contact| Contact {
                    id: contact.id,
                    position: position(contact.axes),
                    origin: contact.origin,
                }),
        );
    }

    fn reset(&mut self) {
        self.packet = Packet::default();
        self.packets.clear();
        self.previous.clear();
        // Type A repeats tracking IDs each frame. The first recovered frame
        // cannot prove a fresh touch; later ID changes or a lift can do so.
        self.recovering = true;
    }
}

enum Protocol {
    TypeB(TypeB),
    TypeA(TypeA),
    Single(SingleTouch),
}

/// Select MT type B with a positive slot count, MT type A with zero slots,
/// or the ABS_X/ABS_Y single-touch protocol with `multitouch == false`.
pub struct Tracker {
    protocol: Protocol,
    button_left: bool,
    dropping: bool,
    frame: Frame,
}

impl Tracker {
    pub fn new(multitouch: bool, slot_count: usize) -> Self {
        let protocol = if !multitouch {
            Protocol::Single(SingleTouch::default())
        } else if slot_count == 0 {
            Protocol::TypeA(TypeA::default())
        } else {
            Protocol::TypeB(TypeB {
                slots: (0..slot_count).map(|_| Slot::default()).collect(),
                selected: Some(0),
            })
        };
        Self {
            protocol,
            button_left: false,
            dropping: false,
            frame: Frame::default(),
        }
    }

    /// Forget state without treating cached coordinates as a new contact.
    /// Type B additionally requires a fresh slot selector after reset.
    pub fn reset(&mut self) {
        self.button_left = false;
        self.dropping = false;
        self.frame.button_left = false;
        self.frame.contacts.clear();
        match &mut self.protocol {
            Protocol::TypeB(state) => state.reset(),
            Protocol::TypeA(state) => state.reset(),
            Protocol::Single(state) => *state = SingleTouch::default(),
        }
    }

    /// Emit only on SYN_REPORT. After SYN_DROPPED, discard through the next
    /// SYN_REPORT (inclusive), emitting an empty frame at that boundary.
    pub fn event(&mut self, event: Event) -> Option<&Frame> {
        if event.kind == EV_SYN && event.code == SYN_DROPPED {
            self.reset();
            self.dropping = true;
            return None;
        }
        if self.dropping {
            if event.kind == EV_SYN && event.code == SYN_REPORT {
                self.dropping = false;
                return Some(&self.frame);
            }
            return None;
        }
        if event.kind == EV_SYN && event.code == SYN_REPORT {
            self.frame.contacts.clear();
            match &mut self.protocol {
                Protocol::TypeB(state) => state.commit(&mut self.frame.contacts),
                Protocol::TypeA(state) => state.commit(&mut self.frame.contacts),
                Protocol::Single(state) => state.commit(&mut self.frame.contacts),
            }
            self.frame.button_left = self.button_left;
            return Some(&self.frame);
        }
        if event.kind == EV_KEY && event.code == BTN_LEFT {
            self.button_left = event.value != 0;
        }
        match &mut self.protocol {
            Protocol::TypeB(state) if event.kind == EV_ABS => state.event(event),
            Protocol::TypeA(state) if event.kind == EV_ABS => state.packet.event(event),
            Protocol::TypeA(state) if event.kind == EV_SYN && event.code == SYN_MT_REPORT => {
                state.finish_packet();
            }
            Protocol::Single(state) => state.event(event),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(tracker: &mut Tracker, kind: u16, code: u16, value: i32) {
        assert!(tracker.event(Event { kind, code, value }).is_none());
    }

    fn report(tracker: &mut Tracker) -> Frame {
        tracker
            .event(Event {
                kind: EV_SYN,
                code: SYN_REPORT,
                value: 0,
            })
            .unwrap()
            .clone()
    }

    fn abs(tracker: &mut Tracker, code: u16, value: i32) {
        send(tracker, EV_ABS, code, value);
    }

    fn key(tracker: &mut Tracker, code: u16, value: i32) {
        send(tracker, EV_KEY, code, value);
    }

    fn mt_contact(tracker: &mut Tracker, id: i32, x: i32, y: i32) {
        abs(tracker, ABS_MT_TRACKING_ID, id);
        abs(tracker, ABS_MT_POSITION_X, x);
        abs(tracker, ABS_MT_POSITION_Y, y);
    }

    #[test]
    fn slots_commit_lift_add_and_keep_origins() {
        let mut tracker = Tracker::new(true, 2);
        mt_contact(&mut tracker, 10, 100, 200);
        let first = report(&mut tracker);
        assert_eq!(first.contacts[0].origin, Some([100.0, 200.0]));
        abs(&mut tracker, ABS_MT_POSITION_X, 120);
        abs(&mut tracker, ABS_MT_SLOT, 1);
        mt_contact(&mut tracker, 20, 300, 400);
        let two = report(&mut tracker);
        assert_eq!(two.contacts.len(), 2);
        assert_eq!(two.contacts[0].position, Some([120.0, 200.0]));
        assert_eq!(two.contacts[0].origin, first.contacts[0].origin);
        abs(&mut tracker, ABS_MT_SLOT, 0);
        abs(&mut tracker, ABS_MT_TRACKING_ID, -1);
        let lifted = report(&mut tracker);
        assert_eq!(lifted.contacts.len(), 1);
        assert_eq!(lifted.contacts[0].id, 20);
        abs(&mut tracker, ABS_MT_POSITION_Y, 600);
        abs(&mut tracker, ABS_MT_POSITION_X, 500);
        abs(&mut tracker, ABS_MT_TRACKING_ID, 30);
        let added = report(&mut tracker);
        assert_eq!(added.contacts[0].origin, Some([500.0, 600.0]));
    }

    #[test]
    fn tracking_id_reuses_kernel_cached_axes_when_unchanged() {
        let mut tracker = Tracker::new(true, 1);
        mt_contact(&mut tracker, 1, 10, 20);
        report(&mut tracker);
        abs(&mut tracker, ABS_MT_TRACKING_ID, -1);
        report(&mut tracker);
        abs(&mut tracker, ABS_MT_POSITION_X, 50);
        abs(&mut tracker, ABS_MT_TRACKING_ID, 2);
        assert_eq!(report(&mut tracker).contacts[0].origin, Some([50.0, 20.0]));
        abs(&mut tracker, ABS_MT_POSITION_X, 70);
        abs(&mut tracker, ABS_MT_TRACKING_ID, 3);
        abs(&mut tracker, ABS_MT_POSITION_Y, 80);
        assert_eq!(report(&mut tracker).contacts[0].origin, Some([70.0, 80.0]));
    }

    #[test]
    fn palms_are_excluded_but_button_and_unknown_contacts_are_reported() {
        let mut tracker = Tracker::new(true, 2);
        abs(&mut tracker, ABS_MT_TOOL_TYPE, MT_TOOL_PALM);
        mt_contact(&mut tracker, 1, 10, 20);
        abs(&mut tracker, ABS_MT_SLOT, 1);
        abs(&mut tracker, ABS_MT_TRACKING_ID, 2);
        key(&mut tracker, BTN_LEFT, 1);
        let frame = report(&mut tracker);
        assert!(frame.button_left);
        assert_eq!(frame.contacts.len(), 1);
        assert_eq!(frame.contacts[0].id, 2);
        assert_eq!(frame.contacts[0].position, None);
        key(&mut tracker, BTN_LEFT, 0);
        abs(&mut tracker, ABS_MT_SLOT, 0);
        abs(&mut tracker, ABS_MT_TOOL_TYPE, 0);
        let frame = report(&mut tracker);
        assert!(!frame.button_left);
        assert_eq!(frame.contacts[0].origin, Some([10.0, 20.0]));
    }

    #[test]
    fn invalid_slot_never_updates_the_previously_selected_slot() {
        let mut tracker = Tracker::new(true, 1);
        mt_contact(&mut tracker, 1, 10, 20);
        report(&mut tracker);
        abs(&mut tracker, ABS_MT_SLOT, 9);
        abs(&mut tracker, ABS_MT_TRACKING_ID, -1);
        assert_eq!(report(&mut tracker).contacts.len(), 1);
    }

    #[test]
    fn drop_suppresses_until_report_and_requires_new_contact_state() {
        let mut tracker = Tracker::new(true, 1);
        mt_contact(&mut tracker, 1, 10, 20);
        key(&mut tracker, BTN_LEFT, 1);
        report(&mut tracker);
        send(&mut tracker, EV_SYN, SYN_DROPPED, 0);
        abs(&mut tracker, ABS_MT_SLOT, 0);
        mt_contact(&mut tracker, 2, 30, 40);
        key(&mut tracker, BTN_LEFT, 1);
        assert_eq!(report(&mut tracker), Frame::default());
        abs(&mut tracker, ABS_MT_POSITION_X, 60);
        assert_eq!(report(&mut tracker), Frame::default());
        abs(&mut tracker, ABS_MT_SLOT, 0);
        mt_contact(&mut tracker, 3, 70, 80);
        assert_eq!(report(&mut tracker).contacts[0].origin, Some([70.0, 80.0]));
    }

    #[test]
    fn single_touch_requires_touch_or_tool_and_keeps_origin_across_buttons() {
        let mut tracker = Tracker::new(false, 0);
        key(&mut tracker, BTN_LEFT, 1);
        abs(&mut tracker, ABS_X, 10);
        abs(&mut tracker, ABS_Y, 20);
        assert!(report(&mut tracker).contacts.is_empty());
        key(&mut tracker, BTN_TOOL_FINGER, 1);
        abs(&mut tracker, ABS_X, 30);
        abs(&mut tracker, ABS_Y, 40);
        let first = report(&mut tracker);
        key(&mut tracker, BTN_LEFT, 0);
        abs(&mut tracker, ABS_X, 50);
        let next = report(&mut tracker);
        assert!(!next.button_left);
        assert_eq!(next.contacts[0].origin, first.contacts[0].origin);
        key(&mut tracker, BTN_TOOL_FINGER, 0);
        assert!(report(&mut tracker).contacts.is_empty());
        key(&mut tracker, BTN_TOUCH, 1);
        assert_eq!(report(&mut tracker).contacts[0].origin, Some([50.0, 40.0]));
    }

    #[test]
    fn single_touch_tool_counts_block_one_finger_and_preserve_primary_origin() {
        let mut tracker = Tracker::new(false, 0);
        key(&mut tracker, BTN_TOUCH, 1);
        abs(&mut tracker, ABS_X, 10);
        abs(&mut tracker, ABS_Y, 20);
        let origin = report(&mut tracker).contacts[0].origin;
        for (code, count) in [
            (BTN_TOOL_DOUBLETAP, 2),
            (BTN_TOOL_TRIPLETAP, 3),
            (BTN_TOOL_QUADTAP, 4),
            (BTN_TOOL_QUINTTAP, 5),
        ] {
            key(&mut tracker, code, 1);
            let frame = report(&mut tracker);
            assert_eq!(frame.contacts.len(), count);
            assert_eq!(frame.contacts[0].origin, origin);
            key(&mut tracker, code, 0);
        }
        assert_eq!(report(&mut tracker).contacts[0].origin, origin);
        key(&mut tracker, BTN_TOUCH, 0);
        assert!(report(&mut tracker).contacts.is_empty());
    }

    #[test]
    fn type_a_ids_preserve_origins_through_reordering_and_partial_coordinates() {
        let mut tracker = Tracker::new(true, 0);
        mt_contact(&mut tracker, 1, 10, 20);
        send(&mut tracker, EV_SYN, SYN_MT_REPORT, 0);
        mt_contact(&mut tracker, 2, 30, 40);
        let first = report(&mut tracker);
        assert_eq!(first.contacts.len(), 2);
        mt_contact(&mut tracker, 2, 35, 45);
        send(&mut tracker, EV_SYN, SYN_MT_REPORT, 0);
        abs(&mut tracker, ABS_MT_POSITION_X, 15);
        abs(&mut tracker, ABS_MT_TRACKING_ID, 1);
        let next = report(&mut tracker);
        assert_eq!(next.contacts[0].id, 2);
        assert_eq!(next.contacts[0].origin, Some([30.0, 40.0]));
        assert_eq!(next.contacts[1].position, Some([15.0, 20.0]));
        assert_eq!(next.contacts[1].origin, Some([10.0, 20.0]));
        assert!(report(&mut tracker).contacts.is_empty());
    }

    #[test]
    fn type_a_anonymous_contacts_are_stable_until_identity_becomes_ambiguous() {
        let mut tracker = Tracker::new(true, 0);
        abs(&mut tracker, ABS_MT_POSITION_X, 10);
        abs(&mut tracker, ABS_MT_POSITION_Y, 20);
        send(&mut tracker, EV_SYN, SYN_MT_REPORT, 0);
        let first = report(&mut tracker);
        abs(&mut tracker, ABS_MT_POSITION_X, 15);
        abs(&mut tracker, ABS_MT_POSITION_Y, 25);
        let next = report(&mut tracker);
        assert_eq!(next.contacts[0].id, first.contacts[0].id);
        assert_eq!(next.contacts[0].origin, first.contacts[0].origin);
        abs(&mut tracker, ABS_MT_POSITION_X, 15);
        abs(&mut tracker, ABS_MT_POSITION_Y, 25);
        send(&mut tracker, EV_SYN, SYN_MT_REPORT, 0);
        abs(&mut tracker, ABS_MT_POSITION_X, 30);
        abs(&mut tracker, ABS_MT_POSITION_Y, 40);
        let two = report(&mut tracker);
        assert_eq!(two.contacts.len(), 2);
        assert!(two.contacts.iter().all(|contact| contact.origin.is_none()));
        abs(&mut tracker, ABS_MT_POSITION_X, 30);
        abs(&mut tracker, ABS_MT_POSITION_Y, 40);
        assert_eq!(report(&mut tracker).contacts[0].origin, None);
        assert!(report(&mut tracker).contacts.is_empty());
        abs(&mut tracker, ABS_MT_POSITION_X, 30);
        abs(&mut tracker, ABS_MT_POSITION_Y, 40);
        assert_eq!(report(&mut tracker).contacts[0].origin, Some([30.0, 40.0]));
    }

    #[test]
    fn type_a_palms_negative_ids_and_recovery_are_conservative() {
        let mut tracker = Tracker::new(true, 0);
        mt_contact(&mut tracker, 1, 10, 20);
        abs(&mut tracker, ABS_MT_TOOL_TYPE, MT_TOOL_PALM);
        send(&mut tracker, EV_SYN, SYN_MT_REPORT, 0);
        abs(&mut tracker, ABS_MT_TRACKING_ID, -1);
        assert!(report(&mut tracker).contacts.is_empty());
        send(&mut tracker, EV_SYN, SYN_DROPPED, 0);
        mt_contact(&mut tracker, 2, 30, 40);
        assert_eq!(report(&mut tracker), Frame::default());
        mt_contact(&mut tracker, 2, 35, 45);
        assert_eq!(report(&mut tracker).contacts[0].origin, None);
        mt_contact(&mut tracker, 2, 40, 50);
        assert_eq!(report(&mut tracker).contacts[0].origin, None);
        mt_contact(&mut tracker, 3, 50, 60);
        assert_eq!(report(&mut tracker).contacts[0].origin, Some([50.0, 60.0]));
    }
}
