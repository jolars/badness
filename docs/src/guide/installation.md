# Installation

Badness is distributed as a single binary, `badness`. The current version is
`{{ badness-version }}`. It is available from several sources:

- **crates.io**: `cargo install badness`
- **Homebrew**: `brew install jolars/tap/badness`
- **npm**: `npm install -g badness` (bundles a prebuilt binary)
- **PyPI**: `uv tool install badness`/`pipx install badness`
- **AUR** (Arch Linux): `yay -S badness-bin` (prebuilt binary)
- **Nix**: `nix shell nixpkgs#badness`; add `pkgs.badness` to
  `environment.systemPackages` for a persistent NixOS installation
- **mise/Aqua**: see [mise and Aqua](#mise-and-aqua) below
- **Prebuilt binaries**: from the [releases
  page](https://github.com/jolars/badness/releases)
- **VS Code/Open VSX**: the
  [**Badness**](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
  extension (also on [Open VSX](https://open-vsx.org/extension/jolars/badness);
  works in Positron and Cursor)

The editor extension bundles a platform-specific `badness` binary and starts the
language server automatically, so you do not need to install the CLI separately.
See [Editor Setup](editor-setup.md) for configuration.

## mise and Aqua

[mise](https://mise.jdx.dev/dev-tools/backends/aqua.html) can install Badness
through its Aqua backend:

```sh
mise use aqua:jolars/badness
```

Commit the resulting `mise.toml` to share the selected version. With
[Aqua](https://aquaproj.github.io/docs/tutorial/) directly, run `aqua init` if
the project has no `aqua.yaml`, then add and install Badness:

```sh
aqua g -i jolars/badness
aqua install
```

Commit `aqua.yaml` to share the selected version. Both tools use the
[`jolars/badness` registry
entry](https://github.com/aquaproj/aqua-registry/tree/main/pkgs/jolars/badness).

## Install Script

The installer selects the release for your platform and installs it in a
directory under your user account. On macOS or Linux:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://badness.dev/install | sh
```

On Windows, run this in PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://badness.dev/install.ps1 | iex"
```

## From Source

Badness is written in Rust. With a Rust toolchain installed, build from a
checkout:

```sh
git clone https://github.com/jolars/badness
cd badness
cargo build --release
```

The build places the binary at `target/release/badness`. Copy it to a directory
on your `PATH`, or run it in place.

To install it into Cargo's bin directory instead:

```sh
cargo install --path .
```

## Verifying the Install

```sh
badness --version
```

This should print `badness {{ badness-version }}`.
