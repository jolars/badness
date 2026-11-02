# Installation

Badness is distributed as a single binary, `badness`. The current version is
`{{ badness-version }}`. Use the installer below or choose a package manager you
already use.

## Install script

The installer selects the release for your platform and installs it in a
directory under your user account:

::: code-group

```sh [macOS/Linux]
curl --proto '=https' --tlsv1.2 -sSf https://badness.dev/install | sh
```

```powershell [Windows (PowerShell)]
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://badness.dev/install.ps1 | iex"
```

:::

The default installation directory is `~/.local/bin` on macOS and Linux, or
`%LOCALAPPDATA%\Programs\badness\bin` on Windows. Make sure that directory is on
your `PATH` before running `badness` from a terminal.

## Homebrew

If you use Homebrew, install Badness from the `jolars/tap` tap:

```sh
brew install jolars/tap/badness
```

## npm

With Node.js and npm installed, install the `badness` command globally:

```sh
npm install -g badness
```

The package includes a prebuilt binary for your platform, so no Rust toolchain
is required.

## PyPI

Install Badness as a command-line tool with uv:

```sh
uv tool install badness
```

If you use pipx instead:

```sh
pipx install badness
```

The PyPI package provides prebuilt binary wheels for supported platforms, so you
do not need a Rust toolchain to install a wheel.

## Cargo

With a Rust toolchain installed, build and install the published release from
crates.io:

```sh
cargo install badness
```

Cargo compiles Badness from source and installs it in Cargo's bin directory.

## AUR

On Arch Linux, install the `badness-bin` package from the AUR. With the `yay`
AUR helper:

```sh
yay -S badness-bin
```

This package installs a prebuilt binary.

## Nix

Start a temporary shell with Badness available:

```sh
nix shell nixpkgs#badness
```

For a persistent NixOS installation, add Badness to your system configuration
and rebuild:

```nix
environment.systemPackages = [ pkgs.badness ];
```

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

## VS Code and Open VSX

Install the [Badness
extension](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
from the VS Code Marketplace or [Open
VSX](https://open-vsx.org/extension/jolars/badness). The extension also works in
Positron and Cursor.

The extension bundles a platform-specific `badness` binary and starts the
language server automatically, so you do not need to install the CLI separately
to use it in your editor. See [editor setup](editor-setup.md#vs-code) for
configuration.

## Mason (Neovim)

If you use Mason in Neovim, install Badness from its registry:

```vim
:MasonInstall badness
```

Then follow the [Neovim setup](editor-setup.md#neovim) to enable the language
server.

## Prebuilt binaries

To install manually, download the archive for your operating system and
architecture from the [releases
page](https://github.com/jolars/badness/releases). Extract it and place the
`badness` binary (`badness.exe` on Windows) in a directory on your `PATH`.

## From source

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

## Verifying the install

```sh
badness --version
```

This should print `badness {{ badness-version }}`.
