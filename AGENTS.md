# Repository Guidelines

## Project Overview

`e-motional-lib` is a Linux touchpad edge-motion daemon/CLI, not a reusable library. A single finger starting in the interior can enter an edge and dwell to produce continuous relative cursor movement; clicking is not required. Physical input remains untouched.

## Architecture & Data Flow

- `src/main.rs` loads configuration, applies CLI overrides, validates, then dispatches setup/device listing or starts the daemon. Configuration is loaded only at startup.
- The daemon is single-threaded: `Reactor` combines epoll, timerfd, signalfd, and udev hotplug. No async runtime or dependency-injection framework; dependencies and mutable state are passed explicitly.
- Pipeline: udev device → `Touchpad::drain` → `Tracker::event` → borrowed `Frame` committed at `SYN_REPORT` → `Motion::update` → fractional `Accumulator::step` → `Injector::emit` through Enigo.
- Each `Touchpad` owns its tracker, motion state, accumulator, and timing. Physical stop frames are processed before timer output in the same readiness batch. Dwell deadlines wake epoll even without new input; the 125 Hz timer runs only while motion is active.
- Preserve safety invariants: physical evdev descriptors are read-only and never grabbed; output is relative movement only, never synthetic buttons, keys, or scrolling. Startup contacts and `SYN_DROPPED` quarantine motion until a full lift.

## Key Directories

- `src/`: daemon, contact tracking, motion calculations, configuration, and interactive calibration; unit tests live beside implementations.
- `packaging/`: installed systemd user-service definition.
- `target/`: ignored build output; keep alternate-backend artifacts separate.

## Development Commands

Run from the repository root:

```sh
cargo +stable build --release --locked
cargo +stable test --locked
cargo +stable clippy --locked --all-targets
cargo +stable fmt --check
cargo +stable run --release --locked -- --list-devices
cargo +stable run --release --locked
```

Clippy/rustfmt are standard Cargo checks, not repository-specific scripts. For Wayland, disable the default backend:

```sh
cargo +stable build --release --locked --no-default-features --features wayland --target-dir target/wayland
cargo +stable test --locked --no-default-features --features wayland --target-dir target/wayland
```

Arch packaging: `makepkg -si`, or `E_MOTIONAL_LIB_BACKEND=wayland makepkg -fsi` from a complete checkout.

## Code Conventions & Common Patterns

- Follow surrounding Rust formatting and naming: `snake_case` modules/functions, `PascalCase` types, `SCREAMING_SNAKE_CASE` constants. No custom rustfmt configuration was found.
- Use `io::Result` for low-level I/O and contextual `Result<_, String>` at orchestration/configuration boundaries; propagate with `?`/`map_err`. Retry interrupted reads, end draining on `WouldBlock`, and remove disconnected devices. Do not silently suppress permission or injection failures.
- Keep event processing allocation-conscious: trackers reuse frames and expose borrowed data; settings are copied values, not shared global mutable state. Pass `Instant` explicitly into motion logic.
- Preserve contact origins across multifinger intervals while resetting motion/dwell. Edge-start contacts remain ineligible; X/Y activation is independent. Cached evdev coordinates across lifts are intentional.
- TOML uses serde defaults and rejects unknown fields. Validate before use/save; preserve atomic temporary-file-and-rename writes and cancellation without saving.

## Important Files

- `src/main.rs`: CLI, startup, reactor, hotplug, and event ordering.
- `src/device.rs` / `src/state.rs`: eligibility, device identity, calibration application, recovery, and single-touch/type-A/type-B tracking.
- `src/edge.rs`: geometry, dwell, hysteresis, velocity, and fractional accumulation.
- `src/config.rs` / `src/setup.rs`: persistence and interactive raw-bound calibration. Configuration is fixed at `$HOME/.config/e-motional-lib.toml`, not `XDG_CONFIG_HOME`; precedence is CLI > file > defaults. Actual default speed scale is **0.3**, despite one stale README passage saying 1.
- `src/inject.rs`: Enigo connection and compile-time backend guards.
- `Cargo.toml` / `Cargo.lock`: dependencies and backend features. `PKGBUILD` explicitly copies Rust source files: update its list when adding/removing modules.
- `packaging/e-motional-lib.service`: graphical-session user service with read-only HOME; run calibration in the foreground. `README.md` contains setup, permissions, and manual acceptance procedures.

## Runtime/Tooling Preferences

- Linux, Rust 2021, stable Rust/Cargo; use the lockfile. Native builds need a C linker, pkg-config, libudev, and libxkbcommon development libraries. No pinned Rust toolchain or manifest `rust-version` is declared.
- Enable **exactly one** backend: default `x11`, or `--no-default-features --features wayland`. Do not use `--all-features`; backend selection is not runtime autodetection.
- Run one instance as the logged-in graphical-session user with evdev read access, not automatically as root. Prefer narrow ACLs; never recommend `chmod 666` on input devices.
- X11 needs display authentication. Experimental Wayland requires compositor virtual-pointer/virtual-keyboard protocol and keymap support; no libei/portal fallback exists.
- Stop the user service before foreground runs or `target/release/e-motional-lib --setup`. Trace logging (`--dbg`) contains sensitive raw input/coordinates; review before sharing.

## Testing & QA

- Built-in Rust `#[test]` modules cover config, edge motion, contact tracking, calibration sampling, and calibration matching. No external test framework, coverage threshold, or CI workflow was found.
- Use focused tests while changing a module, e.g. `cargo +stable test --locked edge::tests`; check both supported feature selections when touching injection or feature-dependent code.
- Reuse synthetic input events/frames, explicit `Instant` offsets, and isolated temporary directories. Avoid sleeps and hardware dependencies in unit tests.
- Unit tests do not establish real cursor injection, physical-device recovery, interactive setup, or service behavior. Use the README manual checklist for these paths; do not present historical README results as current verification.
- Hardware/replay checks can move the real desktop pointer. Use a safe session; replay only into a newly created virtual device, never the physical touchpad. Verify stopping on lift/multifinger input and unchanged physical input.
