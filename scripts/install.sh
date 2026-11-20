#!/usr/bin/env sh
set -e

REPO="HoldenStill/sn-tui"
BINARY="sn-tui"

# Determine OS
OS="$(uname -s)"
case "${OS}" in
    Linux*)     PLATFORM="unknown-linux-musl" ;;
    Darwin*)    PLATFORM="apple-darwin" ;;
    *)
        echo "Error: Unsupported operating system: ${OS}" >&2
        exit 1
        ;;
esac

# Determine Architecture
ARCH="$(uname -m)"
case "${ARCH}" in
    x86_64|amd64)   ARCH_NAME="x86_64" ;;
    aarch64|arm64)  ARCH_NAME="aarch64" ;;
    *)
        echo "Error: Unsupported architecture: ${ARCH}" >&2
        exit 1
        ;;
esac

# Fallback on linux aarch64 to gnu if musl is not built
TARGET="${ARCH_NAME}-${PLATFORM}"
if [ "${OS}" = "Linux" ] && [ "${ARCH_NAME}" = "aarch64" ]; then
    TARGET="aarch64-unknown-linux-gnu"
fi

INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "${INSTALL_DIR}"

ARCHIVE_NAME="${BINARY}-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${ARCHIVE_NAME}"

echo "Fetching latest ${BINARY} (${TARGET}) from GitHub..."
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

if command -v curl >/dev/null 2>&1; then
    curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_DIR}/${ARCHIVE_NAME}"
elif command -v wget >/dev/null 2>&1; then
    wget -qO "${TMP_DIR}/${ARCHIVE_NAME}" "${DOWNLOAD_URL}"
else
    echo "Error: curl or wget is required to download ${BINARY}" >&2
    exit 1
fi

tar -xzf "${TMP_DIR}/${ARCHIVE_NAME}" -C "${TMP_DIR}"

# Find extracted binary
if [ -f "${TMP_DIR}/${BINARY}" ]; then
    SRC_BIN="${TMP_DIR}/${BINARY}"
elif [ -f "${TMP_DIR}/${BINARY}-${TARGET}/${BINARY}" ]; then
    SRC_BIN="${TMP_DIR}/${BINARY}-${TARGET}/${BINARY}"
else
    SRC_BIN="$(find "${TMP_DIR}" -type f -name "${BINARY}" | head -n 1)"
fi

if [ -z "${SRC_BIN}" ] || [ ! -f "${SRC_BIN}" ]; then
    echo "Error: Failed to find ${BINARY} binary inside archive." >&2
    exit 1
fi

install -m 755 "${SRC_BIN}" "${INSTALL_DIR}/${BINARY}"
echo "Successfully installed ${BINARY} to ${INSTALL_DIR}/${BINARY}"

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo ""
        echo "Warning: ${INSTALL_DIR} is not in your \$PATH."
        echo "Add it to your shell configuration:"
        echo "  export PATH=\"\$PATH:${INSTALL_DIR}\""
        ;;
esac
