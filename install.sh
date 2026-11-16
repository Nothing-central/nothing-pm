#!/bin/sh
# nothing — installer for the nth package manager.
#
#   curl -fsSL https://raw.githubusercontent.com/Nothing-central/nothing-pm/main/install.sh | bash
#
# Picks the prebuilt binary for this machine (linux x86_64 or windows x86_64,
# including Git Bash / MSYS on Windows), verifies it against the release
# checksums and drops it on your PATH.
#
# options / environment:
#   --version <tag>     install a specific release, e.g. --version v0.1.0
#   --dir <path>        install into this directory
#   NTH_VERSION         same as --version
#   NTH_INSTALL_DIR     same as --dir
#   NTH_REPO            override the GitHub repo (owner/name)
#   NTH_BASE_URL        override the download base (mirrors)
#   NTH_NO_VERIFY=1     skip checksum verification (not recommended)

set -eu

REPO="${NTH_REPO:-Nothing-central/nothing-pm}"
VERSION="${NTH_VERSION:-}"
INSTALL_DIR="${NTH_INSTALL_DIR:-}"
BASE_URL="${NTH_BASE_URL:-}"

usage() {
    cat <<'EOF'
nothing installer — installs the nth package manager

usage: install.sh [--version <tag>] [--dir <path>]

  --version <tag>   install a specific release (default: latest)
  --dir <path>      install directory (default: /usr/local/bin as root,
                    otherwise ~/.local/bin)
EOF
}

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

have() { command -v "$1" >/dev/null 2>&1; }

# Fetch to stdout, or to a file. curl and wget are treated the same.
get() {
    if have curl; then
        curl -fsSL --retry 3 --connect-timeout 10 "$1"
    elif have wget; then
        wget -q -O - "$1"
    else
        die "need curl or wget to download nth"
    fi
}

try_download() {
    if have curl; then
        curl -fsSL --retry 3 --connect-timeout 10 -o "$2" "$1" 2>/dev/null
    elif have wget; then
        wget -q -O "$2" "$1" 2>/dev/null
    else
        return 1
    fi
}

download() {
    try_download "$1" "$2" || die "failed to download $1"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || die "--version needs a value"
            VERSION="$2"
            shift 2
            ;;
        --dir)
            [ $# -ge 2 ] || die "--dir needs a value"
            INSTALL_DIR="$2"
            shift 2
            ;;
        --help | -h)
            usage
            exit 0
            ;;
        *)
            printf 'unknown argument: %s\n\n' "$1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

# --- what are we on? ---------------------------------------------------
os_raw=$(uname -s 2>/dev/null || echo unknown)
arch_raw=$(uname -m 2>/dev/null || echo unknown)

case "$os_raw" in
    Linux*) os=linux ;;
    Darwin*) os=darwin ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT*) os=windows ;;
    *) die "unsupported operating system: $os_raw" ;;
esac

case "$arch_raw" in
    x86_64 | amd64 | x64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported architecture: $arch_raw" ;;
esac

case "$os-$arch" in
    linux-x86_64) ;;
    windows-x86_64) ;;
    linux-aarch64)
        die "no prebuilt nth for linux-aarch64 yet — build it with: cargo build --release"
        ;;
    windows-aarch64)
        die "no prebuilt nth for windows-aarch64 yet — build it with: cargo build --release"
        ;;
    darwin-*)
        die "no prebuilt nth for macOS yet — build it with: cargo build --release"
        ;;
    *)
        die "no prebuilt nth for $os-$arch"
        ;;
esac

if [ "$os" = "windows" ]; then
    ext=zip
    bin=nth.exe
else
    ext=tar.gz
    bin=nth
fi
asset="nth-$os-$arch.$ext"

# --- which release? ----------------------------------------------------
tag="$VERSION"
if [ -z "$tag" ]; then
    say "looking up the latest release..."
    tag=$(get "https://api.github.com/repos/$REPO/releases/latest" |
        sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
        head -n 1)
    [ -n "$tag" ] || die "could not determine the latest release — pass --version <tag>"
fi

if [ -n "$BASE_URL" ]; then
    url="$BASE_URL/$tag/$asset"
    sums_url="$BASE_URL/$tag/SHA256SUMS"
else
    url="https://github.com/$REPO/releases/download/$tag/$asset"
    sums_url="https://github.com/$REPO/releases/download/$tag/SHA256SUMS"
fi

# --- download, verify, unpack -----------------------------------------
tmp=$(mktemp -d 2>/dev/null || mktemp -d -t nth)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading $asset ($tag)..."
download "$url" "$tmp/$asset"

if [ -z "${NTH_NO_VERIFY:-}" ]; then
    if try_download "$sums_url" "$tmp/SHA256SUMS"; then
        expected=$(awk -v a="$asset" '$2 == a || $2 == "*" a { print $1; exit }' "$tmp/SHA256SUMS")
        if [ -n "$expected" ]; then
            actual=""
            if have sha256sum; then
                actual=$(sha256sum "$tmp/$asset" | awk '{ print $1 }')
            elif have shasum; then
                actual=$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')
            fi
            if [ -n "$actual" ] && [ "$actual" != "$expected" ]; then
                die "checksum mismatch for $asset (expected $expected, got $actual)"
            fi
        fi
    else
        warn "SHA256SUMS not available for $tag — skipping checksum verification"
    fi
fi

say "unpacking..."
cd "$tmp"
case "$ext" in
    zip)
        if have unzip; then
            unzip -oq "$asset"
        else
            tar -xf "$asset"
        fi
        ;;
    *)
        tar -xzf "$asset"
        ;;
esac

[ -f "$bin" ] || die "archive did not contain $bin"
chmod 755 "$bin" 2>/dev/null || true

# --- install -----------------------------------------------------------
if [ -z "$INSTALL_DIR" ]; then
    if [ "$(id -u 2>/dev/null || echo 1)" = "0" ]; then
        INSTALL_DIR=/usr/local/bin
    else
        INSTALL_DIR="${HOME:-.}/.local/bin"
    fi
fi

mkdir -p "$INSTALL_DIR" || die "cannot create $INSTALL_DIR"
target="$INSTALL_DIR/$bin"
cp "$bin" "$target"
chmod 755 "$target" 2>/dev/null || true

installed_version=$("$target" --version 2>/dev/null || true)
say ""
if [ -n "$installed_version" ]; then
    say "✓ installed $installed_version to $target"
else
    say "✓ installed nth to $target"
fi

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        say ""
        say "add $INSTALL_DIR to your PATH:"
        if [ "$os" = "windows" ]; then
            say "  windows: add $INSTALL_DIR to your user PATH (Environment Variables → Path)"
        else
            say "  export PATH=\"$INSTALL_DIR:\$PATH\""
        fi
        ;;
esac
