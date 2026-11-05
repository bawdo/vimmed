#!/bin/sh
# Regenerates the app icons and the README logos from the two source images.
#
#   scripts/make-icons.sh LIGHT_SRC DARK_SRC
#
# LIGHT_SRC is the logo with black text (for light backgrounds), DARK_SRC the
# logo with white text (for dark backgrounds). Both are 1408x768 PNGs with a
# transparent background and the enso centred near (704, 386).
#
# Writes:
#   docs/assets/logo-light.png, docs/assets/logo-dark.png  (README, 512 px)
#   packaging/icons/AppIcon.icns                           (macOS, from DARK_SRC)
#   packaging/icons/hicolor/<s>x<s>/apps/app-icon.png      (Linux, from DARK_SRC)
#
# Needs ImageMagick 7 (`magick`) and, for the .icns, macOS `iconutil`.
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: $0 LIGHT_SRC DARK_SRC" >&2
  exit 2
fi
light=$1
dark=$2
root=$(cd "$(dirname "$0")/.." && pwd)

# A 640 px square around the enso, leaving a small transparent margin.
crop='640x640+384+66'

square() { # src size dest
  magick "$1" -crop "$crop" +repage -filter Lanczos -resize "$2x$2" \
    -define png:exclude-chunks=date,time -strip "PNG32:$3"
}

mkdir -p "$root/docs/assets"
square "$light" 512 "$root/docs/assets/logo-light.png"
square "$dark" 512 "$root/docs/assets/logo-dark.png"

for s in 32 48 64 128 256 512; do
  dir="$root/packaging/icons/hicolor/${s}x${s}/apps"
  mkdir -p "$dir"
  square "$dark" "$s" "$dir/app-icon.png"
done

if command -v iconutil >/dev/null 2>&1; then
  work=$(mktemp -d "${TMPDIR:-/tmp}/icons.XXXXXX")
  set=$work/AppIcon.iconset
  mkdir "$set"
  for s in 16 32 128 256 512; do
    square "$dark" "$s" "$set/icon_${s}x${s}.png"
    square "$dark" "$((s * 2))" "$set/icon_${s}x${s}@2x.png"
  done
  iconutil -c icns -o "$root/packaging/icons/AppIcon.icns" "$set"
  rm -rf "$work"
else
  echo "iconutil not found: packaging/icons/AppIcon.icns left as it was" >&2
fi
