#!/usr/bin/env sh
# Installs Oxed into ~/.local and adds it to the applications menu:
#
#   curl -fsSL https://raw.githubusercontent.com/jkuma/oxed/main/script/install.sh | sh
#
# OXED_VERSION=<x.y.z> installs that release instead of the latest one.
# OXED_BUNDLE_PATH=<oxed-linux-*.tar.gz> installs a bundle built by script/bundle-oxed-linux.
# --uninstall (`| sh -s -- --uninstall`) removes Oxed and keeps the user's settings and data.

set -eu

repository="https://github.com/jkuma/oxed"

main() {
    if [ "$(uname -s)" != "Linux" ]; then
        echo "Oxed only runs on Linux." >&2
        exit 1
    fi

    case "${1:-}" in
        "") install_oxed ;;
        --uninstall) uninstall_oxed ;;
        *)
            echo "Usage: install.sh [--uninstall]" >&2
            exit 1
            ;;
    esac
}

install_oxed() {
    temp="$(mktemp -d "${TMPDIR:-/tmp}/oxed-XXXXXX")"
    trap 'rm -rf "$temp"' EXIT

    if [ -n "${OXED_BUNDLE_PATH:-}" ]; then
        archive="$OXED_BUNDLE_PATH"
    else
        archive="$temp/oxed.tar.gz"
        download "$(release_url)" "$archive"
    fi

    mkdir "$temp/bundle"
    tar -xzf "$archive" -C "$temp/bundle"
    set -- "$temp"/bundle/*.app
    if [ "$#" -ne 1 ] || [ ! -x "$1/bin/oxed" ] || [ ! -x "$1/libexec/oxed-editor" ]; then
        echo "$archive is not an Oxed bundle." >&2
        exit 1
    fi

    app_dir="$HOME/.local/$(basename "$1")"
    applications_dir="$HOME/.local/share/applications"
    mkdir -p "$HOME/.local/bin" "$applications_dir"
    rm -rf "$app_dir"
    mv "$1" "$app_dir"
    ln -sf "$app_dir/bin/oxed" "$HOME/.local/bin/oxed"

    # Absolute paths keep the menu entry working when ~/.local/bin is not on PATH.
    icons_dir="$app_dir/share/icons/hicolor"
    for desktop_file in "$app_dir"/share/applications/*.desktop; do
        icon_name="$(sed -n 's/^Icon=//p' "$desktop_file")"
        icon="$icons_dir/scalable/apps/$icon_name.svg"
        if [ ! -f "$icon" ]; then
            icon="$icons_dir/512x512/apps/$icon_name.png"
        fi
        sed -e "s|Exec=oxed|Exec=$app_dir/bin/oxed|" \
            -e "s|^Icon=.*|Icon=$icon|" \
            "$desktop_file" > "$applications_dir/$(basename "$desktop_file")"
    done
    refresh_applications_menu "$applications_dir"

    if command -v ldd >/dev/null 2>&1; then
        missing="$(ldd "$app_dir/libexec/oxed-editor" 2>/dev/null | sed -n 's/^[[:space:]]*\(.*\) => not found$/\1/p')"
        if [ -n "$missing" ]; then
            echo "Warning: Oxed needs these libraries, which are missing from your system:"
            echo "$missing" | sed 's/^/    /'
            echo "Install them with your package manager, or Oxed will fail to start."
        fi
    fi

    echo "Oxed is installed in $app_dir and available in the applications menu."
    if [ "$(command -v oxed || true)" != "$HOME/.local/bin/oxed" ]; then
        echo "To run 'oxed' from a terminal, add ~/.local/bin to your PATH."
    fi
}

uninstall_oxed() {
    applications_dir="$HOME/.local/share/applications"
    link="$HOME/.local/bin/oxed"
    removed=false

    for app_dir in "$HOME/.local/oxed.app" "$HOME"/.local/oxed-*.app; do
        if [ ! -d "$app_dir" ]; then
            continue
        fi
        for desktop_file in "$app_dir"/share/applications/*.desktop; do
            rm -f "$applications_dir/$(basename "$desktop_file")"
        done
        if [ -L "$link" ]; then
            case "$(readlink "$link")" in
                "$app_dir"/*) rm -f "$link" ;;
            esac
        fi
        rm -rf "$app_dir"
        echo "Removed $app_dir"
        removed=true
    done

    if [ "$removed" = false ]; then
        echo "Oxed is not installed in ~/.local."
        return
    fi
    refresh_applications_menu "$applications_dir"
    echo "Settings in ~/.config/oxed and data in ~/.local/share/oxed are kept."
}

release_url() {
    if [ -n "${OXED_VERSION:-}" ]; then
        echo "$repository/releases/download/v${OXED_VERSION#v}/oxed-linux-$(uname -m).tar.gz"
    else
        echo "$repository/releases/latest/download/oxed-linux-$(uname -m).tar.gz"
    fi
}

download() {
    url="$1"
    destination="$2"
    if command -v curl >/dev/null 2>&1; then
        set -- curl -fL --progress-bar -o "$destination" "$url"
    elif command -v wget >/dev/null 2>&1; then
        set -- wget -O "$destination" "$url"
    else
        echo "Install curl or wget to download Oxed." >&2
        exit 1
    fi
    echo "Downloading $url"
    if ! "$@"; then
        echo "Could not download $url." >&2
        exit 1
    fi
}

refresh_applications_menu() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$1" ||
            echo "Warning: could not refresh the applications menu cache." >&2
    fi
}

main "$@"
