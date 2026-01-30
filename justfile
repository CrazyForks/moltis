# Default recipe (runs when just is called without arguments)
default:
    @just --list

# Format Rust code
format:
    cargo +nightly fmt --all

# Check if code is formatted
format-check:
    cargo +nightly fmt -- --check

# Lint Rust code using clippy
lint:
    cargo clippy --bins --tests --benches --examples --all-features --all-targets -- -D warnings

# Build the project
build:
    cargo build

# Build in release mode
build-release:
    cargo build --release

# Build Debian package for the current architecture
deb: build-release
    cargo deb -p moltis-cli --no-build

# Build Debian package for amd64
deb-amd64:
    cargo build --release --target x86_64-unknown-linux-gnu
    cargo deb -p moltis-cli --no-build --target x86_64-unknown-linux-gnu

# Build Debian package for arm64
deb-arm64:
    cargo build --release --target aarch64-unknown-linux-gnu
    cargo deb -p moltis-cli --no-build --target aarch64-unknown-linux-gnu

# Build Debian packages for all architectures
deb-all: deb-amd64 deb-arm64

# Build Arch package for the current architecture
arch-pkg: build-release
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
    ARCH=$(uname -m)
    PKG_DIR="target/arch-pkg"
    rm -rf "$PKG_DIR"
    mkdir -p "$PKG_DIR/usr/bin"
    cp target/release/moltis "$PKG_DIR/usr/bin/moltis"
    chmod 755 "$PKG_DIR/usr/bin/moltis"
    cat > "$PKG_DIR/.PKGINFO" <<PKGINFO
    pkgname = moltis
    pkgver = ${VERSION}-1
    pkgdesc = Rust version of moltbot
    url = https://docs.molt.bot/
    arch = ${ARCH}
    license = MIT
    PKGINFO
    cd "$PKG_DIR"
    fakeroot -- tar --zstd -cf "../../moltis-${VERSION}-1-${ARCH}.pkg.tar.zst" .PKGINFO usr/
    echo "Built moltis-${VERSION}-1-${ARCH}.pkg.tar.zst"

# Build Arch package for x86_64
arch-pkg-x86_64:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release --target x86_64-unknown-linux-gnu
    VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
    PKG_DIR="target/arch-pkg-x86_64"
    rm -rf "$PKG_DIR"
    mkdir -p "$PKG_DIR/usr/bin"
    cp target/x86_64-unknown-linux-gnu/release/moltis "$PKG_DIR/usr/bin/moltis"
    chmod 755 "$PKG_DIR/usr/bin/moltis"
    cat > "$PKG_DIR/.PKGINFO" <<PKGINFO
    pkgname = moltis
    pkgver = ${VERSION}-1
    pkgdesc = Rust version of moltbot
    url = https://docs.molt.bot/
    arch = x86_64
    license = MIT
    PKGINFO
    cd "$PKG_DIR"
    fakeroot -- tar --zstd -cf "../../moltis-${VERSION}-1-x86_64.pkg.tar.zst" .PKGINFO usr/
    echo "Built moltis-${VERSION}-1-x86_64.pkg.tar.zst"

# Build Arch package for aarch64
arch-pkg-aarch64:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release --target aarch64-unknown-linux-gnu
    VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
    PKG_DIR="target/arch-pkg-aarch64"
    rm -rf "$PKG_DIR"
    mkdir -p "$PKG_DIR/usr/bin"
    cp target/aarch64-unknown-linux-gnu/release/moltis "$PKG_DIR/usr/bin/moltis"
    chmod 755 "$PKG_DIR/usr/bin/moltis"
    cat > "$PKG_DIR/.PKGINFO" <<PKGINFO
    pkgname = moltis
    pkgver = ${VERSION}-1
    pkgdesc = Rust version of moltbot
    url = https://docs.molt.bot/
    arch = aarch64
    license = MIT
    PKGINFO
    cd "$PKG_DIR"
    fakeroot -- tar --zstd -cf "../../moltis-${VERSION}-1-aarch64.pkg.tar.zst" .PKGINFO usr/
    echo "Built moltis-${VERSION}-1-aarch64.pkg.tar.zst"

# Build Arch packages for all architectures
arch-pkg-all: arch-pkg-x86_64 arch-pkg-aarch64

# Build all Linux packages (deb + arch) for all architectures
packages-all: deb-all arch-pkg-all
