# e-motional-lib

Keep moving the pointer when one finger reaches the edge of a touchpad, with or
without a click held down. This is a Linux system service written in Rust, not a
reusable library despite its name.

Physical touchpad events remain untouched: the service reads selected evdev
devices without grabbing them. Extra relative pointer motion goes through a
separate, minimal uinput device. It never synthesizes clicks, keyboard input,
scrolling, or gestures. There are no X11/Wayland build variants, display-server
credentials, input-group additions, or udev permission rules.

## Install

Requirements:

- Linux with a running **systemd 253 or newer** and systemd-logind.
- A local graphical X11 or Wayland user session on **seat0**.
- The uinput kernel module/device and the standard `modprobe@.service` unit,
  with existing device permissions allowing root to read selected touchpads and
  open `/dev/uinput`.
- For source installation: a system-wide stable Rust/Cargo toolchain, a C linker,
  pkg-config, libudev and libsystemd development files, curl, and standard GNU
  userland tools. A Rust toolchain installed only under somebody's home directory
  is deliberately unavailable to the isolated builder.
- Internet access to GitHub and Cargo's dependency registry, and a controlling
  terminal for interactive calibration.

On Arch Linux, install prerequisites with:

```sh
sudo pacman -S --needed base-devel rust pkgconf systemd curl
```

The installer loads `uinput` if its device is missing. At service startup the
broker requests the distribution's standard `modprobe@uinput.service` unit.
Neither step writes module-loading configuration or changes device permissions,
accounts, groups, ACLs, or udev rules. A kernel/security policy that prevents
loading or access causes a failure, not an automatic policy change.

Install and calibrate, then explicitly enable the service:

```sh
curl -fsSL https://raw.githubusercontent.com/DocRAID/e-motional-lib/main/install.sh | sudo sh
sudo systemctl enable --now e-motional-lib
```

The first command is interactive even when piped: calibration reads `/dev/tty`.
Select the intended touchpad, lift all fingers, sweep one finger across its entire
surface including all four edges and corners, finish collection with Enter, and
choose horizontal/vertical edge percentages. Both axes need a measured range.
Repeat setup later to authorize additional touchpads.

### What the installer does

1. Requires root, checks prerequisites and protected destination paths, and
   refuses unmanaged binaries, modified managed files, symlinks, unsafe existing
   configuration paths, conflicting system units/drop-ins, and a conflicting
   static `e-motional-lib` account or group. NSS identities are checked after
   stopping a managed installation's DynamicUser. It does not repair permissions.
2. Resolves upstream `main` to one GitHub commit and downloads that source archive
   and the three units from the same revision over HTTPS. There are no assumed
   prebuilt release assets.
3. Extracts and compiles with `cargo build --release --locked` in a disposable
   **DynamicUser** system service. Cargo dependencies, build scripts, proc macros,
   and source extraction do not run as root. The builder has a private writable
   workspace and temporary directory, no home/device access, and network access
   for dependency downloads. Only its binary output is copied into root's staging
   area; the workspace is removed when the build unit stops.
4. Installs the binary and system units, and records their SHA-256 hashes in a
   private installer manifest. Replacement inodes preserve existing managed-file
   ownership, permissions/ACLs, and extended attributes. On managed upgrades it
   stops only this project's system units before replacement/calibration and
   leaves them stopped.
5. Reloads systemd's unit definitions and runs:

   ```sh
   /usr/bin/e-motional-lib --setup --config-path /etc/e-motional-lib/config.toml < /dev/tty
   ```

   A new configuration directory is root-owned mode `0700`; a new configuration
   file is root-owned mode `0600`. Atomic replacement preserves existing owners,
   modes, POSIX ACLs, and SELinux/Smack labels; inability to preserve these aborts
   the save rather than weakening policy. Existing system configuration must
   already have those private modes,
   be root-owned, and not be symlinked/hard-linked; other paths are rejected.
   Cancelled setup does not save; with no existing configuration the installer
   reports failure. With an existing configuration, cancellation leaves it intact.

The installer **does not enable or start services**, including after upgrades.
If it fails after copying some files, services remain stopped; an interrupted
upgrade may require manually reconciling those project files against the
manifest before retrying. It refuses an existing installation it cannot prove
it owns. Do not mix this installer with a package-manager installation.

The curl command grants the downloaded installer root authority. To review it
first, download it to a file, inspect it, then run `sudo sh ./install.sh`.
HTTPS and a pinned commit keep a single install consistent, but are not a
release-signature or reproducible-build guarantee. You trust this repository,
GitHub, your toolchain, and the locked Cargo dependency sources. The installed
program subsequently runs a small root broker and root setup code.

### Arch package / development checkout

Repository: `git@github.com:DocRAID/e-motional-lib.git`.

```sh
git clone git@github.com:DocRAID/e-motional-lib.git
cd e-motional-lib
makepkg -si
sudo mkdir -m 700 /etc/e-motional-lib   # only if it does not already exist
sudo e-motional-lib --setup --config-path /etc/e-motional-lib/config.toml
sudo systemctl enable --now e-motional-lib
```

The PKGBUILD needs the full checkout, copies all Rust source modules, and installs
system units under `/usr/lib/systemd/system`, not user units. Its default build
directory is `target/makepkg` to avoid colliding with the repository's `src/`.
It has a normal `cargo test` check function for package builders; do not run
`makepkg` as root. For compilation only, use `cargo build --release --locked`.
No backend environment variable or Enigo feature selection remains.

An old user service is not automatically disabled or deleted. Before migrating,
stop it yourself in the owning user's session:

```sh
systemctl --user disable --now e-motional-lib.service
# Only if you used the former project name:
systemctl --user disable --now libinput-edgemotion.service
```

Remove old manually installed artifacts only when you know they belong to this
project; use the package manager for packaged files. Existing user configurations
are neither moved nor changed. Recalibrate the new root-owned system
configuration: its device entries are now a security allowlist, not optional
fallback calibration.

## How it works

1. A socket-activated **root broker** opens only calibrated touchpad identities
   from `/etc/e-motional-lib/config.toml`. Discovery requires touchpad tagging,
   seat0, and non-keyboard capabilities. No configured devices means an error,
   not permission to observe everything. It handles hotplug without changing
   physical device nodes.
2. The broker reads contact frames and sends bounded, validated contact snapshots
   and geometry/settings to an unprivileged worker. It retains the evdev and
   uinput descriptors; neither is passed to the worker.
3. The **DynamicUser worker** calculates edge motion and returns relative
   movement requests over its one inherited connection. The broker validates
   and limits requests before emitting `REL_X`, `REL_Y`, and `SYN_REPORT` through
   its virtual pointer. No physical input is rewritten or blocked.

Exactly one non-palm finger must start outside every edge zone, enter an edge,
and remain there for the dwell interval (default 150 ms). Clicking is irrelevant
to activation: clicking, releasing, and click-dragging do not themselves reset
motion. Edge-start contacts do not activate. Default edge zones are 5% per side
on each axis. Deeper movement into the edge increases speed; corner motion can
activate both axes. Inward hysteresis avoids flicker around a boundary.

Lifting, a second finger, leaving the active zone, removal, invalid input or stale IPC,
or losing an eligible session stops extra motion. Startup, input overflow, and
session transitions require a full lift before fresh contacts can activate.
Physical input continues to work independently of either service process.

MT type A/type B and supported single-touch devices use the existing contact
tracking and calibration logic. Unknown contacts count against the single-finger
condition. Measured axis resolution is used when plausible; otherwise geometry
uses a proportional fallback, so millimetre-based limits are estimates.
Relative movement is accelerated/scaled by the desktop's input stack; it is not
guaranteed to equal screen pixels or to cross a screen in a fixed time.

## Permissions and trust boundary

The only privileged runtime component is the device broker. It runs as root
with an empty capability set, no new privileges, read-only system mounts, hidden
homes, a private device namespace, restricted device access, restricted address
families, and syscall/namespace hardening. It relies on existing root device DAC
permissions, not `CAP_DAC_OVERRIDE`, file capabilities, setuid, or broad user
membership in `input`.

The socket `/run/e-motional-lib/broker.sock` is root-owned `0600` inside a
root-owned `0700` directory. systemd connects as root using `OpenFile=` and passes
the connected descriptor to the worker; the worker cannot open the path itself.
The broker authenticates that root peer and the worker authenticates the broker.
The socket activates the broker independently, avoiding a worker/broker startup
dependency cycle. It is not a public IPC service or an unrestricted FD broker.

| Process | Reads/accesses | Writes/modifies |
| --- | --- | --- |
| DynamicUser worker | Executable/shared libraries, private process/device metadata, inherited broker connection and validated snapshots; no host config, home, evdev, uinput, or user-session sockets | Broker movement messages, journal output, private temporary space; no persistent host files |
| Root broker | Root system config; selected `/dev/input` nodes; read-only `/sys`, `/run/udev`, `/run/systemd/{seats,sessions,users}`; inherited listening socket | `/dev/uinput` virtual relative movement and journal output; no persistent host config, physical evdev writes, or system/user D-Bus connections |
| Root interactive setup | Candidate touchpad metadata and read-only input events; chosen config | Atomic selected calibration/config save, preserving existing ownership, mode, POSIX ACLs, and SELinux/Smack labels |
| Installer/build unit | Fixed upstream source/dependencies; system toolchain; isolated unprivileged build workspace | Installer's binary/three units/hash manifest, new config directory, setup config; build workspace/private temp and journal |

The socket's runtime directory and node are created by the root system manager.
The broker's private `/etc`, `/run`, and `/var` expose only the config, loader
cache, broker runtime path, udev data, and read-only logind metadata needed for
operation. Administrative D-Bus and private systemd sockets stay hidden; the
broker uses sd-login metadata and its monitor, not a bus connection. The worker's
corresponding host trees and `/sys` are hidden. Both can read the
installed executable and libraries; the sandbox is not a separate OS image.
`DynamicUser=yes` allocates a temporary identity rather than creating a persistent
account. Package users must also avoid an existing static account named
`e-motional-lib`; systemd otherwise reuses static identities.

The build DynamicUser writes only its new `/run/e-motional-lib-build-ID/`
workspace (`home`, `cargo`, and `source`) and private temporary files. It reads
the downloaded archive through systemd's read-only credentials directory.
Root staging uses a new private `/var/tmp/e-motional-lib-install.XXXXXXXX`
directory; the installer also holds `/run/e-motional-lib-install.lock`.
These temporary project resources are removed on normal completion/failure.

This separation reduces the worker's authority; it does not make root broker,
installer, dependency, or kernel bugs harmless. A compromised worker can request
bounded pointer motion while known single-contact/session state permits it:
at most 128 relative counts per axis per output, with at least 8 ms between
outputs. High speed settings are capped by this safety limit. The broker's
device sandbox covers the input-device class, while application-level
allowlisting limits the nodes actually opened. Raw contact coordinates and
device identities are sensitive; review logs before sharing them.

## Session and desktop compatibility

Motion is enabled only for an active, local, user-class X11/Wayland session on
seat0 reported by logind. Missing/unavailable/inconsistent session information,
remote sessions, greeters, and non-graphical sessions fail closed. Session changes
reset motion and require lifting all fingers.

**There is no reliable lock-screen gate.** A compositor may leave its locked
graphical session active in logind, so extra pointer motion may continue on the
lock screen, like a physical pointer. Missing lock hints are not interpreted as
proof that a session is unlocked. Do not use this service where a universal
lock-screen inactivity guarantee is required.

uinput avoids X11 authentication and Wayland protocol-specific injection, but
the compositor must accept the virtual relative pointer. Wayland/X11 behavior,
acceleration, virtual-device classification, and sandbox features depend on your
kernel and desktop. Multi-seat, remote desktops, containers without the required
system services/devices, and non-logind setups are not supported.

## Configuration and operation

The service always uses `/etc/e-motional-lib/config.toml`. Root setup/list defaults
to that path and rejects other `--config-path` values; non-root setup/list keeps
`$HOME/.config/e-motional-lib.toml` and may select another path explicitly.
Root broker configuration is fixed to the system path: the parent must be
root-owned mode `0700` and the file root-owned mode `0600`, regular, and not
symlinked/hard-linked. Normal invocation is a worker entrypoint and requires the
manager-provided connection; it is not a second standalone root daemon mode.

To recalibrate or add another authorized touchpad:

```sh
sudo systemctl stop e-motional-lib.service e-motional-lib-broker.socket e-motional-lib-broker.service
sudo e-motional-lib --setup --config-path /etc/e-motional-lib/config.toml
sudo systemctl start e-motional-lib
```

Setup selects a device, records current coordinate extrema, and stores its stable
udev/hardware identity in `[[devices]]`, rather than an unstable `eventN` number.
Only that device's calibration is replaced; other entries remain. Sweep the
whole surface: unsampled edges cannot be inferred. `q`, EOF, or interruption
cancels without saving. TOML comments/formatting are regenerated on a successful
save.

Global settings precede any `[[devices]]` tables:

```toml
edge_percent_x = 5.0
edge_percent_y = 5.0
max_speed_scale = 0.3
dwell_ms = 150
# Optional cap on each edge zone's physical width:
# edge_width_mm = 2.0
```

Edge percentages must be strictly between 0 and 50. Speed scale and optional
width cap must be finite positive values. Device ranges must have finite ordered
min/max values. Invalid configuration is an error, not silently replaced with
defaults. Changing settings requires restarting the project service.
At most 32 touchpad identities may be configured. Setup rejects a configuration
that the bounded broker protocol cannot handle.

`--list-devices` prints devices/identities/ranges without injecting movement.
`--setup`/`-s` calibrates. `--edge-width-mm`, `--max-speed-scale`, and `--dwell-ms`
override setup/list settings; with setup, they are saved. Service motion settings
come from the root config, not ad-hoc worker flags. `--verbose` enables debug
logging and `--dbg`/`-d` detailed tracing where supported. `RUST_LOG` controls
logging; traces may contain raw coordinates and timing.

```sh
sudo e-motional-lib --list-devices --config-path /etc/e-motional-lib/config.toml
sudo systemctl status e-motional-lib e-motional-lib-broker.socket e-motional-lib-broker.service
sudo journalctl -u e-motional-lib -u e-motional-lib-broker -f
sudo systemctl restart e-motional-lib
sudo systemctl disable --now e-motional-lib
```

If there is no extra motion, check the journal, configured device identity,
`ID_INPUT_TOUCHPAD`/seat tagging, available `/dev/uinput`, root DAC permissions,
and the active logind session. Lift fully and begin in the centre after startup
or a session change. Do not solve failures with `chmod`, group additions, ACL
changes, `xhost`, or display credentials. Investigate policies on the particular
system rather than broadening input access.

## Upgrades and removal

Rerun the source installer for a managed source upgrade, review calibration,
then explicitly start the service again. It replaces only hash-matching managed
files and never overwrites locally edited units or a third-party binary. For
customization, maintain the installation yourself; the conservative installer
refuses system-unit overrides/drop-ins rather than silently bypassing them.
For an Arch installation, upgrade/remove through pacman.

For source-installed removal, first stop and disable the main service:

```sh
sudo systemctl disable --now e-motional-lib
sudo systemctl stop e-motional-lib-broker.socket e-motional-lib-broker.service
sudo sha256sum --check /var/lib/e-motional-lib-installer/manifest
```

Only if the manifest matches and these are still the installer-owned files,
remove `/usr/bin/e-motional-lib` and the three files
`/usr/lib/systemd/system/e-motional-lib{,-broker}.service` and
`/usr/lib/systemd/system/e-motional-lib-broker.socket`. Remove the installer-owned
`/var/lib/e-motional-lib-installer/manifest` and its empty directory, then run
`sudo systemctl daemon-reload`. Keep `/etc/e-motional-lib/config.toml` unless you
explicitly want to erase calibration; remove its directory only when empty.
Do not remove unrelated files, legacy user units, existing permission rules,
accounts, groups, or unload uinput used by other applications.

## Verification status

No test suites, installer runs, service starts, or physical pointer-movement
checks were executed while implementing these changes. Deterministic Rust tests
accompany relevant logic, and the PKGBUILD retains its check function, but their
presence is not a claim that they passed. Compilation alone does not validate
systemd activation, hotplug, seat/session transitions, lock-screen behavior, or
actual X11/Wayland pointer movement; these need supported-hardware validation.

## Source layout

- `src/main.rs`: CLI and privileged/unprivileged entrypoint dispatch.
- `src/config.rs`, `src/setup.rs`: validated configuration and calibration.
- `src/device.rs`, `src/state.rs`: read-only discovery, contacts, recovery.
- `src/edge.rs`: geometry, dwell, hysteresis, velocity and accumulation.
- `src/service.rs`, `src/session.rs`: broker/worker and local session gating.
- `src/inject.rs`: minimal relative-motion-only uinput output.
- `packaging/`: system worker, root broker, and root-only activation socket.
- `install.sh`, `PKGBUILD`: isolated source installer and Arch packaging.
- `LICENSE`: MIT.
