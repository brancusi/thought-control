#!/bin/sh
# caretline installer: https://caretline.app/install.sh
#
#   curl -fsSL https://caretline.app/install.sh | sh && caretline demo
#
# Downloads the newest caretline release for this machine (macOS or Linux, arm64 or x86_64)
# from https://github.com/brancusi/thought-control/releases, checks its sha256 and installs
# the `caretline` binary to ~/.local/bin.
#
# Environment:
#   CARETLINE_INSTALL_DIR      where to put the binary (default: ~/.local/bin)
#   CARETLINE_VERSION          a version to install, like 0.2.0 (default: the newest)
#   CARETLINE_NO_MODIFY_PATH=1 don't add the directory to your shell's startup file
#
# Or build it with Rust instead:
#   cargo install --git https://github.com/brancusi/thought-control caretline-app

set -eu

REPO="brancusi/thought-control"

say() { printf '%s\n' "$*"; }
err() { printf 'caretline install: %s\n' "$*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }

fetch() { # url [output]
	if has curl; then
		if [ $# -gt 1 ]; then curl -fsSL --retry 3 -o "$2" "$1"; else curl -fsSL --retry 3 "$1"; fi
	elif has wget; then
		if [ $# -gt 1 ]; then wget -q -O "$2" "$1"; else wget -q -O - "$1"; fi
	else
		err "need curl or wget"
	fi
}

sha256() {
	if has sha256sum; then sha256sum "$1" | cut -d' ' -f1
	elif has shasum; then shasum -a 256 "$1" | cut -d' ' -f1
	elif has openssl; then openssl dgst -sha256 "$1" | sed 's/.*= *//'
	else err "need sha256sum, shasum or openssl to check the download"
	fi
}

target() {
	os="$(uname -s)"
	arch="$(uname -m)"
	case "$os" in
		Darwin) os_part="apple-darwin" ;;
		Linux) os_part="unknown-linux-musl" ;;
		*) err "no prebuilt caretline for $os; try: cargo install --git https://github.com/$REPO caretline-app" ;;
	esac
	case "$arch" in
		arm64 | aarch64) arch_part="aarch64" ;;
		x86_64 | amd64) arch_part="x86_64" ;;
		*) err "no prebuilt caretline for $arch; try: cargo install --git https://github.com/$REPO caretline-app" ;;
	esac
	# An x86_64 shell under Rosetta on Apple silicon still gets the native build.
	if [ "$os" = Darwin ] && [ "$arch_part" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
		arch_part="aarch64"
	fi
	printf '%s-%s' "$arch_part" "$os_part"
}

latest() {
	# The newest caretline-v* release (the repo publishes other things too).
	tag="$(fetch "https://api.github.com/repos/$REPO/releases?per_page=50" 2>/dev/null |
		grep -o '"tag_name": *"caretline-v[0-9][^"]*"' | head -n 1 | sed 's/.*"caretline-v\([^"]*\)"/\1/' || true)"
	if [ -z "$tag" ] && has curl; then
		# The API can be rate limited: fall back to the repo's latest release.
		tag="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null |
			sed -n 's#.*/tag/caretline-v\(.*\)$#\1#p' || true)"
	fi
	[ -n "$tag" ] || err "couldn't find a caretline release; set CARETLINE_VERSION, like CARETLINE_VERSION=0.2.0"
	printf '%s' "$tag"
}

# Adds `dir` to PATH in the shell's startup file, once.
add_to_path() {
	dir="$1"
	[ "${CARETLINE_NO_MODIFY_PATH:-}" = 1 ] && return 1
	shell_name="$(basename "${SHELL:-sh}")"
	case "$shell_name" in
		zsh) rc="${ZDOTDIR:-$HOME}/.zshrc"; line="export PATH=\"$dir:\$PATH\"" ;;
		bash)
			if [ "$(uname -s)" = Darwin ]; then rc="$HOME/.bash_profile"; else rc="$HOME/.bashrc"; fi
			line="export PATH=\"$dir:\$PATH\"" ;;
		fish) rc="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/caretline.fish"; line="fish_add_path \"$dir\"" ;;
		*) rc="$HOME/.profile"; line="export PATH=\"$dir:\$PATH\"" ;;
	esac
	if [ -f "$rc" ] && grep -F "$line" "$rc" >/dev/null 2>&1; then
		printf '%s' "$rc"
		return 0
	fi
	mkdir -p "$(dirname "$rc")" 2>/dev/null || return 1
	{ printf '\n# caretline (https://caretline.app)\n%s\n' "$line"; } >>"$rc" 2>/dev/null || return 1
	printf '%s' "$rc"
}

main() {
	has tar || err "need tar"
	has uname || err "need uname"
	target="$(target)"
	version="${CARETLINE_VERSION:-}"
	version="${version#v}"
	version="${version#caretline-v}"
	[ -n "$version" ] || version="$(latest)"
	dir="${CARETLINE_INSTALL_DIR:-$HOME/.local/bin}"

	name="caretline-$version-$target"
	base="https://github.com/$REPO/releases/download/caretline-v$version"
	tmp="$(mktemp -d 2>/dev/null || mktemp -d -t caretline)"
	trap 'rm -rf "$tmp"' EXIT INT TERM

	say "caretline $version for $target"
	fetch "$base/$name.tar.gz" "$tmp/$name.tar.gz" || err "couldn't download $base/$name.tar.gz"
	fetch "$base/$name.tar.gz.sha256" "$tmp/$name.tar.gz.sha256" || err "couldn't download the checksum"
	want="$(cut -d' ' -f1 <"$tmp/$name.tar.gz.sha256")"
	got="$(sha256 "$tmp/$name.tar.gz")"
	[ -n "$want" ] && [ "$want" = "$got" ] || err "checksum mismatch for $name.tar.gz (expected $want, got $got)"
	say "  sha256 ok  $got"

	tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
	[ -f "$tmp/$name/caretline" ] || err "the archive has no caretline binary"
	mkdir -p "$dir" || err "can't create $dir"
	# Replace through a temporary name, so a running caretline is never half-written.
	cp "$tmp/$name/caretline" "$dir/.caretline.new"
	chmod 755 "$dir/.caretline.new"
	mv -f "$dir/.caretline.new" "$dir/caretline"
	say "  installed  $dir/caretline"

	case ":$PATH:" in
		*":$dir:"*) on_path=1 ;;
		*) on_path=0 ;;
	esac
	if [ "$on_path" = 0 ]; then
		if rc="$(add_to_path "$dir")"; then
			say "  PATH       added $dir to $rc (new terminals pick it up)"
		fi
		say ""
		say "$dir isn't on this shell's PATH yet. For this terminal, run:"
		say ""
		say "  export PATH=\"$dir:\$PATH\""
	fi

	say ""
	say "Next:"
	say "  caretline demo            a guided tour, learned by doing"
	say "  caretline demo scenes     ASCII animations running inside the editor"
	say "  caretline demo agent      a scripted agent co-editing beside you"
	say "  caretline notes.md        edit a file (--outline for lists and tasks)"
	say ""
	say "Docs: https://caretline.app/docs/"
}

main "$@"
