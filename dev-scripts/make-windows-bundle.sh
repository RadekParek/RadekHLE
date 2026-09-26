#!/bin/sh
set -e

# Bundles the MetalHLE 0.1 executable with the basic set of files needed for
# MetalHLE 0.1 to run (the same ones found in the macOS .app bundle or Android APK).
# This does not prepare a full release.

if [[ $# == 1 ]]; then
    PATH_TO_BINARY="$1"
    shift

    rm -rf metalhle_windows_bundle
    mkdir metalhle_windows_bundle
    cp $PATH_TO_BINARY metalhle_windows_bundle/
    cp -r ../touchHLE_dylibs metalhle_windows_bundle/
    cp -r ../touchHLE_fonts metalhle_windows_bundle/
    cp -r ../touchHLE_default_options.txt metalhle_windows_bundle/
    cp -r ../res/MetalHLE_v7_wallpaper.png metalhle_windows_bundle/
else
    echo "Incorrect usage."
    exit 1
fi
