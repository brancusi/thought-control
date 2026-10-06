#!/bin/sh
# Install thc (thought-central) on a Mac:
#
#   curl -fsSL https://github.com/brancusi/thought-control-releases/releases/latest/download/install.sh | sh
#
# Puts thc in ~/.local/bin, then runs `thc setup --all --yes`: the vault (~/thought), PATH, the
# background daemon (a LaunchAgent) and agent skills. If thc is currently a link into
# Thought Central.app, it migrates: the standalone binary replaces the link, the app's login item
# goes, and thc's own runs the daemon.
#
# Checks before anything is installed: the tarball's sha256 against the manifest, every manifest
# signature against the ed25519 key built into thc, and that the binary reports the manifest's version.
set -eu

MANIFEST="${THC_INSTALL_MANIFEST:-https://github.com/brancusi/thought-control-releases/releases/latest/download/latest.json}"
DEST_DIR="$HOME/.local/bin"

say() { printf '%s\n' "$*"; }
fail() { printf 'thc install: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || fail "the installer is macOS-only for now"
case "$(uname -m)" in
  arm64) target=aarch64-apple-darwin ;;
  x86_64) target=x86_64-apple-darwin ;;
  *) fail "unsupported CPU $(uname -m)" ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$MANIFEST" -o "$tmp/latest.json" || fail "couldn't fetch $MANIFEST"
get() { plutil -extract "$1" raw -o - "$tmp/latest.json" 2>/dev/null || fail "the manifest has no $1"; }
version="$(get version)"
url="$(get "targets.$target.url")"
sha="$(get "targets.$target.sha256")"

say "thc $version ($target)"
curl -fsSL "$url" -o "$tmp/$(basename "$url")" || fail "couldn't download $url"
got="$(shasum -a 256 "$tmp/$(basename "$url")" | awk '{print $1}')"
[ "$got" = "$sha" ] || fail "checksum mismatch: got $got, the manifest says $sha"
tar -xzf "$tmp/$(basename "$url")" -C "$tmp" thc || fail "couldn't unpack thc"

"$tmp/thc" release-tool verify "$tmp/latest.json" "$tmp" >/dev/null || fail "the manifest's signatures don't verify"
[ "$("$tmp/thc" --version)" = "thc $version" ] || fail "the binary isn't thc $version"
say "  verified: checksum, Apple signature, update signature"

mkdir -p "$DEST_DIR"
link="$DEST_DIR/thc"
if [ -L "$link" ] && readlink "$link" | grep -q '\.app/Contents/'; then
  say "  thc is a link into Thought Central.app: moving to the standalone thc"
  exec "$tmp/thc" setup --migrate-from-app
fi
cp "$tmp/thc" "$DEST_DIR/.thc.installing"
chmod 755 "$DEST_DIR/.thc.installing"
mv -f "$DEST_DIR/.thc.installing" "$link"
say "  installed $link"
exec "$link" setup --all --yes
