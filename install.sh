#!/bin/sh
# Run as root; Cargo and all downloaded build scripts run only as a DynamicUser.
set -eu
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH
umask 077

fail() { printf 'e-motional-lib: %s\n' "$*" >&2; exit 1; }
[ "$(id -u)" = 0 ] || fail 'Run this installer as root (sudo sh).'
[ "$#" = 0 ] || fail 'This installer takes no arguments.'
for command in curl systemctl systemd-run tar cargo rustc cc pkg-config sha256sum stat mktemp install awk; do
    command -v "$command" >/dev/null 2>&1 || fail "Missing prerequisite: $command"
done
[ -d /run/systemd/system ] || fail 'A running systemd system manager is required.'
version=$(systemctl --version | awk 'NR == 1 { print $2 }')
case "$version" in ''|*[!0-9]*) fail 'Cannot determine systemd version.' ;; esac
[ "$version" -ge 253 ] || fail 'systemd 253 or newer is required (OpenFile= support).'
pkg-config --exists libudev libsystemd || fail 'Install libudev and libsystemd development files.'
[ -c /dev/uinput ] || fail '/dev/uinput is missing; load the uinput kernel module using your distribution policy, then retry.'
[ -r /dev/tty ] && [ -w /dev/tty ] || fail 'Interactive calibration requires a controlling terminal (/dev/tty).'

if awk -F: '$1 == "e-motional-lib" { found = 1 } END { exit !found }' /etc/passwd /etc/group; then
    fail 'A static e-motional-lib account/group already exists; reconcile it yourself rather than reusing it as a DynamicUser.'
fi
# Never repair permissions or follow a pre-existing privileged path through symlinks.
secure_path() {
    [ ! -L "$1" ] || fail "Refusing symlink: $1"
    [ "$(stat -c %u "$1")" = 0 ] || fail "Not root-owned: $1"
    mode=$(stat -c %a "$1")
    [ "$((0$mode & 022))" = 0 ] || fail "Writable by non-root: $1"
}
for directory in /usr /usr/bin /usr/lib /usr/lib/systemd /usr/lib/systemd/system /etc /var /var/lib /run; do
    [ -d "$directory" ] || fail "Required directory is missing: $directory"
    secure_path "$directory"
done
state=/var/lib/e-motional-lib-installer
manifest=$state/manifest
config=/etc/e-motional-lib/config.toml
units='e-motional-lib.service e-motional-lib-broker.service e-motional-lib-broker.socket'
if [ -e "$state" ] || [ -L "$state" ]; then
    [ -d "$state" ] || fail "Not a directory: $state"
    secure_path "$state"
    [ "$(stat -c %a "$state")" = 700 ] || fail "Expected private installer directory: $state"
fi
if [ -e "$manifest" ] || [ -L "$manifest" ]; then
    [ -f "$manifest" ] || fail "Not a regular file: $manifest"
    secure_path "$manifest"
    [ "$(stat -c %a "$manifest")" = 600 ] || fail "Expected private manifest: $manifest"
    [ "$(awk 'END { print NR }' "$manifest")" = 4 ] || fail 'Invalid installer manifest.'
fi
if [ -e /etc/e-motional-lib ] || [ -L /etc/e-motional-lib ]; then
    [ -d /etc/e-motional-lib ] || fail 'Configuration parent is not a directory.'
    secure_path /etc/e-motional-lib
    [ "$(stat -c %a /etc/e-motional-lib)" = 700 ] || fail 'Existing configuration directory must already have mode 0700; no permissions were changed.'
fi
if [ -e "$config" ] || [ -L "$config" ]; then
    [ -f "$config" ] || fail 'Configuration is not a regular file.'
    secure_path "$config"
    [ "$(stat -c %a "$config")" = 600 ] || fail 'Existing configuration must already have mode 0600; no permissions were changed.'
    [ "$(stat -c %h "$config")" = 1 ] || fail 'Refusing hard-linked configuration.'
fi
if [ -e /run/e-motional-lib ] || [ -L /run/e-motional-lib ]; then
    [ -d /run/e-motional-lib ] || fail 'Broker runtime path is not a directory.'
    secure_path /run/e-motional-lib
    [ "$(stat -c %g /run/e-motional-lib)" = 0 ] || fail 'Existing broker runtime directory must belong to group root.'
    [ "$(stat -c %a /run/e-motional-lib)" = 700 ] || fail 'Existing broker runtime directory must already have mode 0700.'
fi
if [ -e /run/e-motional-lib/broker.sock ] || [ -L /run/e-motional-lib/broker.sock ]; then
    [ -S /run/e-motional-lib/broker.sock ] || fail 'Broker socket path has a conflicting file.'
    secure_path /run/e-motional-lib/broker.sock
    [ "$(stat -c %g /run/e-motional-lib/broker.sock)" = 0 ] || fail 'Existing broker socket must belong to group root.'
    [ "$(stat -c %a /run/e-motional-lib/broker.sock)" = 600 ] || fail 'Existing broker socket must already have mode 0600.'
fi
for unit in $units; do
    for base in /etc/systemd/system /run/systemd/system /run/systemd/transient /usr/local/lib/systemd/system; do
        for path in "$base/$unit" "$base/$unit.d"; do
            [ ! -e "$path" ] && [ ! -L "$path" ] || fail "Remove or reconcile the conflicting system unit/override yourself: $path"
        done
    done
done

check_destination() {
    destination=$1
    if [ -f "$manifest" ]; then
        [ -f "$destination" ] || fail "Managed file is missing: $destination"
        secure_path "$destination"
        case "$destination" in /usr/bin/e-motional-lib) required_mode=755 ;; *) required_mode=644 ;; esac
        [ "$(stat -c %a "$destination")" = "$required_mode" ] || fail "Managed file mode was changed; refusing to replace: $destination"
        expected=$(awk -v path="$destination" '$2 == path { print $1 }' "$manifest")
        case "$expected" in ''|*[!0-9a-f]*) fail 'Invalid installer manifest digest.' ;; esac
        [ "${#expected}" = 64 ] || fail 'Invalid installer manifest digest length.'
        actual=$(sha256sum "$destination" | awk '{ print $1 }')
        [ "$actual" = "$expected" ] || fail "Managed file was modified; refusing to overwrite: $destination"
    else
        [ ! -e "$destination" ] && [ ! -L "$destination" ] || fail "Unmanaged file exists; use its package manager or reconcile it yourself: $destination"
    fi
}
check_destinations() {
    check_destination /usr/bin/e-motional-lib
    for unit in $units; do check_destination "/usr/lib/systemd/system/$unit"; done
}

lock=/run/e-motional-lib-install.lock
mkdir -m 700 "$lock" 2>/dev/null || fail "Installer lock exists: $lock (remove only after verifying no installer is running)."
work=
staging=
cleanup() {
    [ -z "$staging" ] || rm -rf -- "$staging"
    [ -z "$work" ] || rm -rf -- "$work"
    rmdir "$lock"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
check_destinations
work=$(mktemp -d /var/tmp/e-motional-lib-install.XXXXXXXX)

# Resolve main once so the source and service units come from the same revision.
origin=https://api.github.com/repos/DocRAID/e-motional-lib/commits/main
curl --proto '=https' --tlsv1.2 -fsSL "$origin" -o "$work/revision.json"
revision=$(awk -F '"' '/"sha"[[:space:]]*:/ { print $4; exit }' "$work/revision.json")
case "$revision" in ''|*[!0-9a-f]*) fail 'GitHub returned an invalid source revision.' ;; esac
[ "${#revision}" = 40 ] || fail 'GitHub returned an invalid source revision length.'
printf 'Building trusted upstream revision %s as a disposable DynamicUser.\n' "$revision"
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL \
    "https://codeload.github.com/DocRAID/e-motional-lib/tar.gz/$revision" -o "$work/source.tar.gz"
for unit in $units; do
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL \
        "https://raw.githubusercontent.com/DocRAID/e-motional-lib/$revision/packaging/$unit" -o "$work/$unit"
    [ -s "$work/$unit" ] || fail "Empty upstream unit: $unit"
done
build_name=e-motional-lib-build-${work##*.}
# The manager opens stdout; the build user cannot access the root-owned staging directory.
# Source extraction, dependency downloads, proc macros and build.rs all stay unprivileged.
systemd-run --quiet --wait --collect --unit="$build_name" \
    --property=DynamicUser=yes \
    --property="RuntimeDirectory=$build_name" \
    --property=RuntimeDirectoryMode=0700 \
    --property="LoadCredential=source.tar.gz:$work/source.tar.gz" \
    --property="StandardOutput=file:$work/binary" \
    --property=StandardError=journal \
    --property=ProtectSystem=strict --property=ProtectHome=yes \
    --property=PrivateTmp=yes --property=PrivateDevices=yes \
    --property=NoNewPrivileges=yes --property=CapabilityBoundingSet= \
    --property=RestrictSUIDSGID=yes --property=RestrictNamespaces=yes \
    --property=ProtectKernelTunables=yes --property=ProtectKernelModules=yes \
    --property=ProtectKernelLogs=yes --property=ProtectControlGroups=yes \
    --property=RestrictAddressFamilies='AF_UNIX AF_INET AF_INET6' \
    /bin/sh -ec '
        export PATH=/usr/local/bin:/usr/bin:/bin
        export HOME="$RUNTIME_DIRECTORY/home" CARGO_HOME="$RUNTIME_DIRECTORY/cargo"
        mkdir -p "$HOME" "$CARGO_HOME" "$RUNTIME_DIRECTORY/source"
        tar -xzf "$CREDENTIALS_DIRECTORY/source.tar.gz" --strip-components=1 -C "$RUNTIME_DIRECTORY/source"
        cd "$RUNTIME_DIRECTORY/source"
        cargo build --release --locked >&2
        test -f target/release/e-motional-lib
        test ! -L target/release/e-motional-lib
        cat target/release/e-motional-lib
    ' || fail "Unprivileged compilation failed; inspect journalctl -u $build_name."
[ -s "$work/binary" ] || fail 'Compilation produced no binary.'

# Refuse conflicts again before changing files; never replace a third-party installation.
check_destinations
if [ -f "$manifest" ]; then
    printf 'Stopping the project system units for upgrade and interactive calibration; they will remain stopped.\n'
    systemctl stop e-motional-lib.service e-motional-lib-broker.socket e-motional-lib-broker.service
fi
replace_file() {
    source=$1 destination=$2 mode=$3
    staging=$(mktemp -d "${destination%/*}/.e-motional-lib.XXXXXXXX")
    install -m "$mode" "$source" "$staging/file"
    mv -fT "$staging/file" "$destination"
    rmdir "$staging"
    staging=
}
replace_file "$work/binary" /usr/bin/e-motional-lib 755
for unit in $units; do
    replace_file "$work/$unit" "/usr/lib/systemd/system/$unit" 644
done
if [ ! -d "$state" ]; then mkdir -m 700 "$state"; fi
{
    sha256sum /usr/bin/e-motional-lib
    for unit in $units; do sha256sum "/usr/lib/systemd/system/$unit"; done
} > "$work/manifest"
replace_file "$work/manifest" "$manifest" 600
systemctl daemon-reload
if [ ! -d /etc/e-motional-lib ]; then mkdir -m 700 /etc/e-motional-lib; fi
printf 'Select and calibrate the touchpad(s) this service may observe.\n'
/usr/bin/e-motional-lib --setup --config-path /etc/e-motional-lib/config.toml < /dev/tty
[ -f "$config" ] && [ ! -L "$config" ] || fail 'No system configuration was saved; calibration was cancelled. Run setup before enabling the service.'
secure_path "$config"
printf '\nInstalled. No services were enabled or started.\nRun: sudo systemctl enable --now e-motional-lib\n'
