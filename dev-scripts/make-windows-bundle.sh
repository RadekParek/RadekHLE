#!/bin/sh
set -e

# Bundles the RadekHLE9.0 executable with the basic set of files needed for
# RadekHLE9.0 to run (the same ones found in the macOS .app bundle or Android APK).
# This does not prepare a full release.

if [[ $# == 1 ]]; then
    PATH_TO_BINARY="$1"
    shift

    rm -rf radekhle_windows_bundle
    mkdir radekhle_windows_bundle
    cp $PATH_TO_BINARY radekhle_windows_bundle/
    cp -r ../touchHLE_dylibs radekhle_windows_bundle/
    cp -r ../touchHLE_fonts radekhle_windows_bundle/
    cp -r ../touchHLE_default_options.txt radekhle_windows_bundle/
    cp -r ../res/RadekHLE_v7_wallpaper.png radekhle_windows_bundle/
else
    echo "Incorrect usage."
    exit 1
fi
