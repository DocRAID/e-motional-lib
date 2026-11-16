# Local checkout package: run makepkg -si from the repository root.
# makepkg's default "src/" would overlap Rust sources (including cleanbuild).
if [[ ${BUILDDIR:-$startdir} -ef $startdir ]]; then
    BUILDDIR="$startdir/target/makepkg"
fi
pkgname=e-motional-lib
pkgver=0.1.0
pkgrel=1
pkgdesc='Touchpad edge motion with native cursor output'
arch=('x86_64' 'aarch64')
license=('MIT')
# Replace the former package, rather than leaving two daemons installed.
conflicts=('libinput-edgemotion')
replaces=('libinput-edgemotion')
# Cargo's default release profile intentionally contains no debug symbols.
options=('!debug')
depends=('gcc-libs' 'glibc' 'systemd-libs' 'libxkbcommon')
makedepends=('rust' 'pkgconf')
# The task supplies a complete checkout, not a published release URL.
source=()
sha256sums=()

prepare() {
    mkdir -p "$srcdir/$pkgname/src"
    cp "$startdir/Cargo.toml" "$startdir/Cargo.lock" "$srcdir/$pkgname/"
    cp "$startdir/src/"{main,config,setup,device,state,edge,inject}.rs "$srcdir/$pkgname/src/"
    cd "$srcdir/$pkgname"
    cargo fetch --locked
}

build() {
    cd "$srcdir/$pkgname"
    cargo build --release --frozen --no-default-features --features "${E_MOTIONAL_LIB_BACKEND:-x11}"
}

check() {
    cd "$srcdir/$pkgname"
    cargo test --frozen --no-default-features --features "${E_MOTIONAL_LIB_BACKEND:-x11}"
}

package() {
    install -Dm755 "$srcdir/$pkgname/target/release/$pkgname" "$pkgdir/usr/bin/$pkgname"
    install -Dm644 "$startdir/packaging/e-motional-lib.service" "$pkgdir/usr/lib/systemd/user/e-motional-lib.service"
    install -Dm644 "$startdir/README.md" "$pkgdir/usr/share/doc/$pkgname/README.md"
    install -Dm644 "$startdir/LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
