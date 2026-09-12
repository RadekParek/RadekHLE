# Bundled ANGLE runtime

These arm64-v8a shared libraries are the ANGLE Vulkan runtime used by the Android build. They are packaged as native libraries so the emulator can force ANGLE instead of selecting a vendor OpenGL ES implementation.

Source provenance: the ANGLE 2.1.25902 Android prebuilts carried by the HyperHLE-Fork ANGLE update (`6fdf524e1d2b0b0cf1c99e7b3e53a4f049490a3f`), built from the virgl-angle-termux prebuilts. ANGLE is distributed under its upstream Chromium/ANGLE open-source licences.
