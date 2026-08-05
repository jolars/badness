# AUR packaging

This directory is the source of truth for the
[`badness-bin`](https://aur.archlinux.org/packages/badness-bin) AUR package.
There is no separate AUR mirror repo: the AUR git remote
(`ssh://aur@aur.archlinux.org/badness-bin.git`) is a pure deploy target.

## How it publishes

The `publish-aur.yml` workflow runs automatically after each release's binary
assets are uploaded (chained in `build-and-test.yml`). It rewrites `pkgver`,
`pkgrel`, and the checksums in this `PKGBUILD`, then pushes `PKGBUILD` +
`.SRCINFO` to the AUR via
[KSXGitHub/github-actions-deploy-aur](https://github.com/KSXGitHub/github-actions-deploy-aur),
which also test-builds the package first.

The `pkgver` and checksums committed here are a snapshot of the last release at
the time this file was touched; the workflow always overwrites them at publish
time. They are kept real (not placeholders) so `makepkg -si` from a checkout
works.

Requirements (one-time):

- An AUR account with an SSH public key registered.
- The matching private key stored as the `AUR_SSH_PRIVATE_KEY` repo secret.

The first workflow run claims the `badness-bin` name by pushing to the empty
AUR repo; no manual bootstrap is needed.

## Manual fallback

If CI is unavailable, `task aur:push` (see `scripts/aur_push.sh`) does the same
update locally. It needs `makepkg` on the `PATH` (the `pacman` package on
non-Arch distros) for `.SRCINFO` generation, plus AUR SSH access.

Re-releasing the same version (e.g. a repackaging fix) requires bumping
`pkgrel`: run the workflow manually via `workflow_dispatch` with a `pkgrel`
input, or `task aur:push -- <version> <pkgrel>`.
