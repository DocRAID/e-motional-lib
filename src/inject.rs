use enigo::{Coordinate, Enigo, InputResult, Mouse, Settings};

#[cfg(any(
    all(feature = "x11", feature = "wayland"),
    not(any(feature = "x11", feature = "wayland"))
))]
compile_error!(
    "Select exactly one Enigo backend: default X11, or --no-default-features --features wayland"
);

const BACKEND: &str = if cfg!(feature = "wayland") {
    "Wayland"
} else {
    "X11"
};

pub struct Injector(Enigo);

impl Injector {
    pub fn new() -> Result<Self, String> {
        log::debug!(
            "connecting Enigo {BACKEND}: DISPLAY={:?} WAYLAND_DISPLAY={:?} XDG_RUNTIME_DIR={:?}",
            std::env::var_os("DISPLAY"),
            std::env::var_os("WAYLAND_DISPLAY"),
            std::env::var_os("XDG_RUNTIME_DIR")
        );
        let settings = Settings {
            linux_delay: 0,
            // This daemon only moves the pointer; it never owns keyboard/button state.
            release_keys_when_dropped: false,
            ..Settings::default()
        };
        let cursor = Enigo::new(&settings).map_err(|error| {
            let hint = if cfg!(feature = "wayland") {
                "run in a Wayland session exposing virtual-pointer/virtual-keyboard protocols with WAYLAND_DISPLAY and XDG_RUNTIME_DIR; this build does not include libei/portal support"
            } else {
                "run as the logged-in X11 desktop user with DISPLAY and XAUTHORITY; use the Wayland build for a supported Wayland compositor"
            };
            format!("Enigo {BACKEND} connection failed: {error}; {hint}")
        })?;
        log::info!("cursor output: Enigo {BACKEND}, relative motion only");
        Ok(Self(cursor))
    }

    pub fn emit(&mut self, delta: [i32; 2]) -> InputResult<()> {
        if delta != [0, 0] {
            self.0.move_mouse(delta[0], delta[1], Coordinate::Rel)?;
            log::trace!(
                "Enigo {BACKEND} moved pointer dx={} dy={}",
                delta[0],
                delta[1]
            );
        }
        Ok(())
    }
}
