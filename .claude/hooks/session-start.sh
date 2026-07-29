#!/usr/bin/env bash
#
# SessionStart hook for Claude Code on the web.
#
# Local development uses devenv/Nix (`devenv.nix`), which provides the Rust
# toolchain plus the auxiliary tooling the Taskfile drives. The hosted web
# container ships Rust, Python, and Node but nothing else, so this script
# provisions the gap and warms the build: the container image is snapshotted
# once the hook finishes, so a slow step here is paid once, not per session.
#
# Everything is pinned (exact release tag + SHA-256, a pinned nixpkgs snapshot)
# and idempotent. Nothing here is *required* for a correct session -- `cargo
# build`/`test`/`clippy`/`fmt` all work against the stock container -- so every
# optional step warns and continues rather than failing the session.
set -euo pipefail

REPO="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/badness"
BIN="$CACHE/bin"
NIX_VERSION="2.24.10"

# Prebuilt release binaries. api.github.com is *not* reachable from this
# container (403), but release *asset* URLs are -- so pin exact tags rather
# than resolving `latest`, which keeps the environment reproducible anyway.
TASK_VERSION="3.45.4"
TASK_URL="https://github.com/go-task/task/releases/download/v$TASK_VERSION/task_linux_amd64.tar.gz"
TASK_SHA256="4367eba04abcbcb407578d18d2439ee32604a872419601abec76a829c797fb82"

# mdbook 0.5.x is required, not merely preferred: docs/doc-utils is built
# against mdbook-preprocessor 0.5, and a 0.4.x host speaks a different
# preprocessor protocol ("missing field `items`"), so `mdbook build` fails.
MDBOOK_VERSION="0.5.4"
MDBOOK_URL="https://github.com/rust-lang/mdBook/releases/download/v$MDBOOK_VERSION/mdbook-v$MDBOOK_VERSION-x86_64-unknown-linux-musl.tar.gz"
MDBOOK_SHA256="5222beabd3e37dc5be0d18ff99b79058469354db5c220153a1b92db5ba12be89"

# A nixpkgs snapshot pinned by URL. `github:NixOS/nixpkgs` is unusable here:
# the web container's GitHub access is scoped to this repository, so flake
# inputs resolving through api.github.com return 403. releases.nixos.org serves
# the same tree as a plain tarball and is reachable, so fetch it from there.
NIXPKGS_URL="https://releases.nixos.org/nixos/unstable/nixos-26.11pre1042126.624af665418d/nixexprs.tar.xz"

log() { printf '[session-start] %s\n' "$*" >&2; }

# Only the hosted container needs this; a local devenv shell already has it all.
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  log "not a remote session; leaving the toolchain to devenv"
  exit 0
fi

mkdir -p "$BIN"
ENV_LINES="$CACHE/env.sh.tmp"
: > "$ENV_LINES"
printf 'export PATH="%s:$PATH"\n' "$BIN" >> "$ENV_LINES"

# Fetch a pinned release tarball and drop a single binary into $BIN.
#   install_tool <bin> <version> <url> <sha256> <member-path-in-tarball>
install_tool() {
  local bin="$1" version="$2" url="$3" want="$4" member="$5"
  local stamp="$CACHE/$bin-$version.stamp"
  local tmp have

  # Fast path: a previous run installed this exact version and the binary
  # survives in the container snapshot.
  if [ -x "$BIN/$bin" ] && [ -f "$stamp" ]; then
    return 0
  fi

  log "installing $bin $version"
  tmp="$(mktemp -d)"

  if ! curl -sSL --retry 3 --max-time 300 -o "$tmp/pkg.tar.gz" "$url"; then
    log "downloading $bin failed"
    rm -rf "$tmp"
    return 1
  fi

  have="$(sha256sum "$tmp/pkg.tar.gz" | cut -d' ' -f1)"
  if [ "$have" != "$want" ]; then
    log "$bin checksum mismatch (want $want, got $have) -- refusing to install"
    rm -rf "$tmp"
    return 1
  fi

  if ! tar -xzf "$tmp/pkg.tar.gz" -C "$tmp" "$member"; then
    log "extracting $member from the $bin tarball failed"
    rm -rf "$tmp"
    return 1
  fi

  install -m 0755 "$tmp/$member" "$BIN/$bin"
  : > "$stamp"
  rm -rf "$tmp"
  return 0
}

install_nix() {
  # Written unconditionally: /nix can survive in a cached container image while
  # /etc is rebuilt, and without this config nix refuses to build (the
  # container runs as root with no nixbld group, so single-user mode must be
  # selected).
  mkdir -p /etc/nix
  cat > /etc/nix/nix.conf <<'CONF'
build-users-group =
experimental-features = nix-command flakes
substituters = https://cache.nixos.org
sandbox = false
CONF

  NIX_BIN="$(echo /nix/store/*-nix-"$NIX_VERSION"/bin 2>/dev/null | tr ' ' '\n' | head -1)"
  if [ -x "$NIX_BIN/nix" ]; then
    return 0
  fi

  # The usual installer scripts (nixos.org/nix/install,
  # install.determinate.systems) are not reachable from this container, but the
  # release tarballs on releases.nixos.org are. Install from the tarball.
  log "installing nix $NIX_VERSION"
  local tmp
  tmp="$(mktemp -d)"
  curl -sSL --retry 3 --max-time 300 \
    -o "$tmp/nix.tar.xz" \
    "https://releases.nixos.org/nix/nix-$NIX_VERSION/nix-$NIX_VERSION-x86_64-linux.tar.xz" || {
    log "downloading nix failed"
    rm -rf "$tmp"
    return 1
  }
  tar -xf "$tmp/nix.tar.xz" -C "$tmp" || { rm -rf "$tmp"; return 1; }

  "$tmp/nix-$NIX_VERSION-x86_64-linux/install" --no-daemon --no-channel-add >/dev/null 2>&1 || true
  rm -rf "$tmp"

  NIX_BIN="$(echo /nix/store/*-nix-"$NIX_VERSION"/bin 2>/dev/null | tr ' ' '\n' | head -1)"
  if [ ! -x "$NIX_BIN/nix" ]; then
    log "nix install did not produce a usable binary"
    return 1
  fi
  return 0
}

# Build a nix expression and echo the store path that actually carries
# `bin/<want>`. A derivation with several outputs (texlive splits off `-man`)
# makes `--print-out-paths` emit one line per output, so picking the first
# would hand back a path with no bin/ at all.
#   nix_build_bin <binary-name> <nix-expression>
nix_build_bin() {
  local want="$1" expr="$2" out path
  out="$(nix build --impure --no-link --print-out-paths --expr "$expr")" || return 1
  while IFS= read -r path; do
    if [ -x "$path/bin/$want" ]; then
      printf '%s\n' "$path"
      return 0
    fi
  done <<< "$out"
  return 1
}

# --- Task runner and docs toolchain -----------------------------------------
# `task` drives every documented workflow (Taskfile.yml); `mdbook` builds the
# book under docs/. Neither is needed to build or test the crate.
install_tool task "$TASK_VERSION" "$TASK_URL" "$TASK_SHA256" task \
  || log "go-task unavailable -- run the underlying cargo commands directly"
install_tool mdbook "$MDBOOK_VERSION" "$MDBOOK_URL" "$MDBOOK_SHA256" mdbook \
  || log "mdbook unavailable -- 'task docs:rules' still regenerates the rule pages"

# --- Rust -------------------------------------------------------------------
# Warm the cargo registry so the first build in the session is not a cold
# fetch. This also materializes the toolchain rust-toolchain.toml pins,
# including the wasm32-unknown-unknown target. The texlab parser/syntax crates
# are git dependencies, so this reaches github.com as well as crates.io.
log "fetching cargo dependencies"
cargo fetch --manifest-path "$REPO/Cargo.toml"

# Compile every dependency and target -- lib, bin, examples, integration tests,
# benches -- so the session's first `cargo test` is an incremental rebuild of
# badness alone rather than of the whole dependency graph.
log "building all targets"
cargo build --manifest-path "$REPO/Cargo.toml" --all-targets --all-features --quiet

# clippy's `--all-targets --all-features` artifacts are a separate cache from
# the plain build, and AGENTS.md makes a clean clippy run a precondition for
# committing -- so warm that too.
log "warming the clippy cache"
cargo clippy --manifest-path "$REPO/Cargo.toml" --all-targets --all-features --quiet >/dev/null 2>&1 \
  || log "clippy warm-up reported problems; run it directly to see them"

# docs/doc-utils is a separate crate (the mdbook preprocessor plus the
# canonical/sitemap binaries). book.toml marks the preprocessor `optional`, so
# an unbuilt one silently ships raw `{{ badness-version }}` markers rather than
# failing -- build it here the way CI does.
log "building the docs preprocessor"
cargo build --manifest-path "$REPO/docs/doc-utils/Cargo.toml" --quiet \
  || log "docs/doc-utils failed to build -- 'mdbook build docs' will skip the preprocessor"

# --- TeX Live (optional) ----------------------------------------------------
# Only `task bib-fields:check`/`:sync` needs a TeX installation: it locates
# biblatex's data model via `kpsewhich blx-dm.def`. Nothing in `cargo test`
# touches these, so a failure here leaves the session fully usable.
#
# Two bundles, both from the pinned nixpkgs snapshot:
#
#   TeX Live -- `task bib-fields:check/:sync` locates biblatex's data model via
#     `kpsewhich blx-dm.def`, and `task bench` measures against latexindent,
#     chktex, and lacheck. texliveSmall plus those packages is a few hundred MB,
#     versus many GB for a full scheme.
#
#   Dev tools -- the benchmark comparators and stats backend the `bench` and
#     `profile` skills drive (tex-fmt, hyperfine, jq) plus the dependency gates
#     CI enforces (cargo-audit, cargo-deny, cargo-machete).
#
# `cargo flamegraph`/`perf` are deliberately absent: profiling needs kernel
# perf-event access this container does not grant, so the `profile` skill's
# flamegraph step cannot work here. Its wall-time measurements still can.
#
#   provision_nix_bundle <marker-name> <probe-binary> <description> <nix-expr>
provision_nix_bundle() {
  local marker="$CACHE/$1.path" probe="$2" desc="$3" expr="$4"
  local path
  path="$(cat "$marker" 2>/dev/null || true)"

  # Fast path: a previous run built it and the store path survives in the
  # container snapshot, so skip nix evaluation entirely.
  if [ -z "$path" ] || [ ! -x "$path/bin/$probe" ]; then
    install_nix || return 1
    export PATH="$NIX_BIN:$PATH"

    log "building $desc from the pinned nixpkgs snapshot"
    path="$(nix_build_bin "$probe" "$expr")" || {
      log "nix build of $desc failed"
      return 1
    }
    printf '%s\n' "$path" > "$marker"
  fi

  printf 'export PATH="%s/bin:$PATH"\n' "$path" >> "$ENV_LINES"
  return 0
}

if provision_nix_bundle texlive kpsewhich "texliveSmall + biblatex/latexindent/chktex/lacheck" \
  "with import (fetchTarball \"$NIXPKGS_URL\") {}; texliveSmall.withPackages (ps: [ ps.biblatex ps.latexindent ps.chktex ps.lacheck ])"; then
  log "texlive ready (kpsewhich, latexindent, chktex, lacheck)"
else
  log "texlive unavailable -- 'task bib-fields:check/:sync' needs it (or pass"
  log "'scripts/gen_bib_fields.py --def PATH'); 'task bench' drops those comparators"
fi

if provision_nix_bundle dev-tools hyperfine "the benchmark and dependency-audit tools" \
  "with import (fetchTarball \"$NIXPKGS_URL\") {}; symlinkJoin { name = \"badness-dev-tools\"; paths = [ hyperfine jq tex-fmt cargo-audit cargo-deny cargo-machete ]; }"; then
  log "dev tools ready (hyperfine, jq, tex-fmt, cargo-audit, cargo-deny, cargo-machete)"
else
  log "dev tools unavailable -- 'task bench' falls back to a shell timing loop"
fi

# Put nix itself on PATH whenever it is installed -- including the fast path,
# where no bundle was rebuilt -- so an ad-hoc `nix build` is available for
# anything this hook does not pre-provision.
NIX_BIN="$(echo /nix/store/*-nix-"$NIX_VERSION"/bin 2>/dev/null | tr ' ' '\n' | head -1)"
if [ -x "$NIX_BIN/nix" ]; then
  printf 'export PATH="%s:$PATH"\n' "$NIX_BIN" >> "$ENV_LINES"
fi

# --- VS Code extension (optional) -------------------------------------------
# CI type-checks and bundles editors/code; the deps are small and the npm
# registry is in the container's no-proxy list, so install them up front.
if [ -f "$REPO/editors/code/package-lock.json" ]; then
  log "installing editors/code node dependencies"
  npm ci --prefix "$REPO/editors/code" --silent \
    || log "npm ci in editors/code failed -- the extension will not build"
fi

# --- Publish to the session environment -------------------------------------
if [ -n "${CLAUDE_ENV_FILE:-}" ] && [ -s "$ENV_LINES" ]; then
  while IFS= read -r line; do
    grep -qxF "$line" "$CLAUDE_ENV_FILE" 2>/dev/null || printf '%s\n' "$line" >> "$CLAUDE_ENV_FILE"
  done < "$ENV_LINES"
fi
rm -f "$ENV_LINES"

# Known gap, so nobody burns a session chasing it: `task pkg-names:check/:sync`
# fetches the TeX Live package database from ftp.math.utah.edu, which this
# container's network policy blocks (403 through the proxy).
log "ready: cargo build/test/clippy/fmt, task, mdbook"
