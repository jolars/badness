#!/usr/bin/env bash
# Manual fallback for the publish-aur.yml workflow: update badness-bin on the
# AUR from packaging/aur/PKGBUILD. Needs makepkg (for .SRCINFO generation) and
# SSH access to the AUR.
#
# Usage: scripts/aur_push.sh [version] [pkgrel]
#   version  defaults to the latest v* git tag, without the leading v
#   pkgrel   defaults to 1; bump when re-releasing the same version
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
version=${1:-$(git -C "$repo_root" describe --tags --abbrev=0 --match 'v*')}
version=${version#v}
pkgrel=${2:-1}

command -v makepkg >/dev/null || {
  echo "error: makepkg not found (install pacman/pacman-contrib)" >&2
  exit 1
}

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

echo "Publishing badness-bin $version-$pkgrel to the AUR"

git clone ssh://aur@aur.archlinux.org/badness-bin.git "$workdir/aur"
cp "$repo_root/packaging/aur/PKGBUILD" "$workdir/aur/PKGBUILD"
cd "$workdir/aur"

for target in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
  curl -fsSL -o "badness-$target.tar.gz" \
    "https://github.com/jolars/badness/releases/download/v$version/badness-$target.tar.gz"
done

sha_x64=$(sha256sum badness-x86_64-unknown-linux-gnu.tar.gz | awk '{print $1}')
sha_arm64=$(sha256sum badness-aarch64-unknown-linux-gnu.tar.gz | awk '{print $1}')
rm badness-*.tar.gz

sed -i \
  -e "s/^pkgver=.*/pkgver=$version/" \
  -e "s/^pkgrel=.*/pkgrel=$pkgrel/" \
  -e "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('$sha_x64')/" \
  -e "s/^sha256sums_aarch64=.*/sha256sums_aarch64=('$sha_arm64')/" \
  PKGBUILD

makepkg --printsrcinfo >.SRCINFO

if git diff --quiet PKGBUILD .SRCINFO; then
  echo "AUR package already at $version-$pkgrel; nothing to push"
  exit 0
fi

git add PKGBUILD .SRCINFO
git commit -m "Update to v$version"
git push
echo "Pushed badness-bin $version-$pkgrel"
