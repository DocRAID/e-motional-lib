use evdev::{uinput::VirtualDevice, AttributeSet, EventType, InputEvent, KeyCode, PropType, RelativeAxisCode};
use std::io;

pub struct Injector(VirtualDevice);

impl Injector {
    pub fn new() -> Result<Self, String> {
        let axes: AttributeSet<RelativeAxisCode> =
            [RelativeAxisCode::REL_X, RelativeAxisCode::REL_Y].into_iter().collect();
        // Mouse classification needs BTN_LEFT capability; no button events are emitted.
        let keys: AttributeSet<KeyCode> = [KeyCode::BTN_LEFT].into_iter().collect();
        let properties: AttributeSet<PropType> = [PropType::POINTER].into_iter().collect();
        let device = VirtualDevice::builder()
            .and_then(|builder| builder.with_relative_axes(&axes))
            .and_then(|builder| builder.with_keys(&keys))
            .and_then(|builder| builder.with_properties(&properties))
            .and_then(|builder| builder.name("e-motional-lib virtual pointer").build())
            .map_err(|error| format!("cannot create /dev/uinput relative pointer: {error}"))?;
        Ok(Self(device))
    }

    pub fn emit(&mut self, delta: [i32; 2]) -> io::Result<()> {
        if delta == [0, 0] {
            return Ok(());
        }
        self.0.emit(&[
            InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, delta[0]),
            InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, delta[1]),
        ])
    }
}
