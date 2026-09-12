#!/bin/sh
set -e

# Bundles the RadekHLE9.0 executable with the basic set of files needed for
# RadekHLE9.0 to run (the same ones found in the macOS .app bundle or Android APK).
# This does not prepare a full release.

if [ "$#" -eq 1 ]; then
    PATH_TO_BINARY="$1"
    shift

    rm -rf radekhle_linux_bundle
    mkdir radekhle_linux_bundle
    cp $PATH_TO_BINARY radekhle_linux_bundle/
    cp -r ../touchHLE_dylibs radekhle_linux_bundle/
    cp -r ../touchHLE_fonts radekhle_linux_bundle/
    cp -r ../touchHLE_default_options.txt radekhle_linux_bundle/
    cp -r ../res/RadekHLE_v7_wallpaper.png radekhle_linux_bundle/
else
    echo "Incorrect usage."
    exit 1
fi
