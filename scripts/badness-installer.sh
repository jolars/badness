#!/usr/bin/env sh
set -eu

REPO="${BADNESS_REPO:-jolars/badness}"
INSTALL_DIR="${BADNESS_INSTALL_DIR:-$HOME/.local/bin}"
TAG="${BADNESS_TAG:-}"
VERIFY="${BADNESS_VERIFY_CHECKSUM:-true}"

os="$(uname -s)"
arch="$(uname -m)"

# Linux releases ship both glibc and musl builds, so probe the host's libc
# rather than assuming glibc. On musl `ldd --version` writes to stderr and
# exits non-zero, hence the 2>&1 and the tolerance for a failing pipeline.
detect_libc() {
  if [ -n "${BADNESS_LIBC:-}" ]; then
    case "$BADNESS_LIBC" in
    gnu | musl) printf '%s\n' "$BADNESS_LIBC" ;;
    *)
      echo "BADNESS_LIBC must be 'gnu' or 'musl', got '$BADNESS_LIBC'" >&2
      exit 1
      ;;
    esac
    return 0
  fi

  if ldd --version 2>&1 | grep -qi musl; then
    printf 'musl\n'
    return 0
  fi

  # Fallback for images with no ldd at all: look for musl's loader.
  for loader in /lib/ld-musl-*.so.1; do
    if [ -e "$loader" ]; then
      printf 'musl\n'
      return 0
    fi
  done

  printf 'gnu\n'
}

case "$os" in
Linux)
  libc="$(detect_libc)"
  case "$arch" in
  x86_64 | amd64) target="x86_64-unknown-linux-${libc}" ;;
  aarch64 | arm64) target="aarch64-unknown-linux-${libc}" ;;
  *)
    echo "Unsupported Linux architecture: $arch" >&2
    exit 1
    ;;
  esac
  ;;
Darwin)
  case "$arch" in
  x86_64 | amd64) target="x86_64-apple-darwin" ;;
  arm64 | aarch64) target="aarch64-apple-darwin" ;;
  *)
    echo "Unsupported macOS architecture: $arch" >&2
    exit 1
    ;;
  esac
  ;;
*)
  echo "Unsupported operating system: $os" >&2
  exit 1
  ;;
esac

asset="badness-${target}.tar.gz"

resolve_download_url() {
  if [ -n "$TAG" ]; then
    case "$TAG" in
    v* | badness-v*)
      tag_candidates="$TAG"
      ;;
    *)
      tag_candidates="v${TAG} badness-v${TAG}"
      ;;
    esac

    for tag_candidate in $tag_candidates; do
      candidate_url="https://github.com/${REPO}/releases/download/${tag_candidate}/${asset}"
      if curl --proto '=https' --tlsv1.2 -fsSLI "$candidate_url" >/dev/null 2>&1; then
        printf '%s\n' "$candidate_url"
        return 0
      fi
    done

    echo "Could not find release asset ${asset} for BADNESS_TAG='${TAG}' in ${REPO}" >&2
    exit 1
  fi

  api_url="https://api.github.com/repos/${REPO}/releases?per_page=100"
  resolved_url="$(
    curl --proto '=https' --tlsv1.2 -fsSL "$api_url" \
      | tr ',' '\n' \
      | grep 'browser_download_url' \
      | grep -F "/${asset}\"" \
      | sed -E 's/.*"browser_download_url"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/' \
      | sed 's#\\/#/#g' \
      | sed -n '1p'
  )"

  if [ -z "$resolved_url" ]; then
    echo "Could not find a release asset named ${asset} in ${REPO}" >&2
    exit 1
  fi

  printf '%s\n' "$resolved_url"
}

url="$(resolve_download_url)"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT INT TERM

echo "Downloading ${asset}..."
curl --proto '=https' --tlsv1.2 -fLsS "$url" -o "$tmpdir/$asset"

if [ "$VERIFY" = "true" ]; then
  # Prefer the per-asset .sha256 sidecar, falling back to the release-wide
  # SHA256SUMS manifest, since not every release publishes both. Older releases
  # may have neither, in which case warn and continue rather than fail.
  expected=""
  if curl --proto '=https' --tlsv1.2 -fLsS "${url}.sha256" -o "$tmpdir/$asset.sha256"; then
    expected="$(awk '{print $1}' "$tmpdir/$asset.sha256")"
  elif curl --proto '=https' --tlsv1.2 -fLsS "${url%/*}/SHA256SUMS" -o "$tmpdir/SHA256SUMS"; then
    expected="$(awk -v a="$asset" '$2 == a || $2 == "*" a {print $1}' "$tmpdir/SHA256SUMS")"
  fi

  if [ -z "$expected" ]; then
    echo "Warning: no published checksum for ${asset}; skipping verification." >&2
  else
    if command -v sha256sum >/dev/null 2>&1; then
      actual="$(sha256sum "$tmpdir/$asset" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
      actual="$(shasum -a 256 "$tmpdir/$asset" | awk '{print $1}')"
    else
      echo "No sha256sum or shasum available; cannot verify checksum" >&2
      exit 1
    fi
    if [ "$expected" != "$actual" ]; then
      echo "Checksum mismatch for ${asset}" >&2
      echo "  expected: $expected" >&2
      echo "  actual:   $actual" >&2
      exit 1
    fi
    echo "Checksum verified."
  fi
fi

tar -xzf "$tmpdir/$asset" -C "$tmpdir"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmpdir/badness" "$INSTALL_DIR/badness"

echo "Installed badness to $INSTALL_DIR/badness"
case ":$PATH:" in
*":$INSTALL_DIR:"*) ;;
*)
  echo "Note: $INSTALL_DIR is not on PATH."
  ;;
esac
