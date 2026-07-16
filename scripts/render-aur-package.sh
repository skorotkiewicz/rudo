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
license=('MIT')
depends=('gtk4' 'gtk4-layer-shell')
options=(!strip)
install='${pkgname}.install'
source=("rudo-\${pkgver}.tar.gz::https://github.com/${repo_owner}/${repo_name}/releases/download/v\${pkgver}/rudo-\${pkgver}-x86_64-linux.tar.gz")
sha256sums=('${sha256}')

package() {
    install -Dm755 rudo "\${pkgdir}/usr/bin/rudo"
    install -Dm644 rudo.service "\${pkgdir}/usr/lib/systemd/user/rudo.service"
    install -Dm644 README.md "\${pkgdir}/usr/share/doc/\${pkgname}/README.md"
    install -Dm644 LICENSE "\${pkgdir}/usr/share/licenses/\${pkgname}/LICENSE"
}
EOF

cat > "$out_dir/${pkgname}.install" <<'EOF'
post_install() {
    printf '%s\n' \
        ':: Rudo includes a systemd user service for autostart on niri.' \
        '   Enable it as your desktop user with:' \
        '     systemctl --user enable --now rudo.service' \
        '' \
        '   Remove any existing `spawn-at-startup "rudo"` entry first.'
}
EOF
