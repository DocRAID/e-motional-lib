# Repository Guidelines

## Project Overview

`e-motional-lib` adds sustained edge motion to Linux touchpads without grabbing or rewriting physical input. One interior-started finger enters an edge and dwells; clicking is not required. Installation targets systemd system services, not a graphical user's service.

## Architecture & Data Flow

- `src/main.rs` dispatches administrative `--setup`/`--list-devices`, privileged `--broker`, and isolated `--worker` modes. Worker dispatch occurs before any configuration-file access.
- The root broker owns selected evdev descriptors and the uinput pointer. The DynamicUser worker owns geometry, dwell, velocity, and fractional accumulation. Only bounded contact/movement messages cross their UNIX-stream boundary; never pass raw device descriptors to the worker.
- systemd owns the root-only activation socket. `OpenFile=` delegates a connected socket to the worker without changing device permissions, ACLs, or persistent accounts/groups.
- `Config.devices` is the normal-runtime allowlist, not merely optional calibration. Resolve persistent identity before opening a device; accept only configured seat0 touchpads, never keyboard-capable event nodes.
- Preserve full-lift quarantine on startup, overflow, and session transitions. Physical stop frames take precedence over output. Output is relative X/Y movement only: never keys, buttons, wheel events, grabs, or device reconfiguration requested by the worker.
- Session gating uses logind metadata, not user display credentials. Missing/invalid authorization must stop motion, not trigger a permissive fallback.

## Key Directories

- `src/`: CLI, broker/worker transport, session gate, contact tracking, motion, persistence, and interactive calibration.
- `packaging/`: system service and socket units; their sandbox is part of the security boundary.
- `target/`: ignored compiler output, never installed configuration or device state.

## Development Commands

```sh
cargo +stable check --bin e-motional-lib
cargo +stable build --release --locked
cargo +stable clippy --locked --all-targets
cargo +stable fmt --check
sh -n install.sh
```

Tests are user-run during this redesign: do not execute tests, the installer, daemons, or system services without authorization. `cargo +stable test --locked` is available for the user. Arch packaging uses `makepkg -si`; the root installer builds source under a disposable DynamicUser, never as root.

## Code Conventions & Common Patterns

- Follow Rust naming and surrounding formatting. No async runtime, shared mutable globals, or dependency-injection framework: pass owned/borrowed state explicitly.
- Use `io::Result` for low-level I/O and contextual `Result<_, String>` at boundaries. Retry `Interrupted`, stop draining on `WouldBlock`, and remove disconnected devices. Permission, IPC, and injection failures must not be silently ignored.
- Reuse borrowed tracker frames; keep input/motion loops allocation-conscious. Pass `Instant` explicitly into motion logic. Contact origins survive multifinger intervals; motion/dwell does not.
- Validate all worker-originated messages before acting with privilege. Fixed protocol limits, timeouts, session checks, and physical stop guards are security behavior, not optional optimization.
- TOML rejects unknown fields. Atomic saves preserve existing configuration ownership, mode, and POSIX ACLs on the replacement inode. Reject symlinks/hard links; never repair existing permissions automatically.

## Important Files

- `src/service.rs` / `src/protocol.rs`: broker/worker lifecycle, hotplug, event ordering, bounded IPC, and isolated motion.
- `src/device.rs` / `src/state.rs`: device identity, descriptor validation, bootstrap/recovery, and single-touch/type-A/type-B tracking.
- `src/edge.rs`: geometry, edge-origin exclusion, independent-axis dwell/hysteresis, velocity, and fractional accumulation.
- `src/inject.rs` / `src/session.rs`: movement-only uinput output and session authorization.
- `src/config.rs` / `src/setup.rs`: root configuration at `/etc/e-motional-lib/config.toml`, administrative calibration, and atomic persistence. Nonroot administrative use retains the HOME configuration default. `--config-path` selects an explicit path; privileged use requires the fixed system path.
- `install.sh`, `PKGBUILD`, `packaging/`: installation, source staging, system services, socket delegation, and filesystem isolation. Update source staging whenever modules change.
- `README.md`: installation prerequisites, service operation, trust boundaries, and DynamicUser file-access documentation. Keep its claims aligned with actual units/code.

## Runtime/Tooling Preferences

- Linux, stable Rust/Cargo, Rust 2021, systemd 253+, libudev, libsystemd, linker, and pkg-config. Use `Cargo.lock`. No Enigo, X11/Wayland build-feature variants, or user display authentication.
- Never change existing `/dev` modes/ownership/ACLs, udev permission rules, or existing user/group memberships. No setuid binaries, file capabilities, broad DAC-bypass capabilities, or automatic security-policy weakening.
- Root-only configuration directory/file use initial 0700/0600 permissions. Workers must neither read host configuration directly nor write persistent files. New project-owned resources are distinct from permission changes to existing resources.
- Setup selection remains interactive and may inspect eligible candidates. Runtime observes only configured devices. Stop project service/broker/socket units before setup; a directory lock prevents concurrent broker/setup access.
- Trace logging exposes input coordinates/device identities. Do not share unreviewed traces or enable them by default.

## Testing & QA

- Built-in Rust `#[test]` modules use synthetic events/frames, explicit time offsets, and isolated temporary directories. Add deterministic regression coverage for uncertain protocol/security edges, but respect the user's execution prohibition.
- Compilation is not proof of sandbox enforcement, pointer compatibility, session routing, or hardware recovery. Do not claim tests or hardware behavior were verified unless actually authorized and observed.
- Physical/replay checks can move the desktop pointer. The user owns testing for this redesign; never launch those checks implicitly.
