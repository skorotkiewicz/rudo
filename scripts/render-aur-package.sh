#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 6 ]]; then
    echo "usage: $0 <out-dir> <pkgname> <pkgver> <repo-owner> <repo-name> <sha256>"
    exit 1
fi

out_dir="$1"
pkgname="$2"
pkgver="$3"
repo_owner="$4"
repo_name="$5"
sha256="$6"

mkdir -p "$out_dir"

cat > "$out_dir/PKGBUILD" <<EOF
pkgname=${pkgname}
pkgver=${pkgver}
pkgrel=1
pkgdesc='Elegant Wayland dock with niri-aware integration'
arch=('x86_64')
url='https://github.com/${repo_owner}/${repo_name}'
license=('unknown')
depends=('gtk4' 'gtk4-layer-shell')
source=("\${repo_name}-\${pkgver}.tar.gz::https://github.com/${repo_owner}/${repo_name}/releases/download/v\${pkgver}/\${repo_name}-\${pkgver}-x86_64-linux.tar.gz")
sha256sums=('${sha256}')

package() {
    install -Dm755 rudo "\${pkgdir}/usr/bin/rudo"
    install -Dm644 README.md "\${pkgdir}/usr/share/doc/\${repo_name}/README.md" 2>/dev/null || true
}
EOF
