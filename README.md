# RadekHLE9.9

RadekHLE9.9 is a community fork of [touchHLE](https://github.com/touchHLE/touchHLE), an HLE emulator for early iPhone OS applications. It keeps the upstream Rust architecture and adds Android, ARM64, graphics, and compatibility work.

The project is licensed under the Mozilla Public License 2.0. Source and dependency notices are in `LICENSE`, `touchHLE_dylibs/`, and `touchHLE_fonts/`.

## Architecture

The 32-bit path uses the original `Environment` and Dynarmic A32 execution path. ARM64 uses `Environment64`, `Mem64`, the Dynarmic A64 wrapper, and the Rust interpreter because ARM64 has a different register ABI, pointer width, address space, Mach-O format, and callback conventions. `arm64_runtime.rs` is the host-dispatch layer for that environment; it is not a second CPU implementation. Both paths reuse the existing framework, window, audio, and graphics layers wherever their APIs are shared.

Keeping the two execution environments separate prevents 64-bit pointers from entering the 32-bit memory and Objective-C code, while allowing compatibility fixes in shared frameworks to benefit both paths.

## Building

Initialise submodules before building:

```sh
git submodule update --init
RUSTFLAGS="-C link-arg=-latomic" cargo build --release
```

For a headless build check, use `cargo check`. The graphical binary needs an SDL2 display at runtime. Release bundles are created by the scripts in `dev-scripts/`; their downloaded archive names use the `RadekHLE_...` prefix.

## Usage

Place decrypted `.ipa` files or `.app` bundles in `touchHLE_apps/` for the app picker, or pass an app path to the command-line binary. `OPTIONS_HELP.txt` lists the available options.

Only run software you have obtained legally.

## Credits

RadekHLE9.9 builds on touchHLE and the open-source projects listed by the upstream project, including Dynarmic, SDL, OpenAL Soft, Symphonia, rust-macho, and the Rust ecosystem. See `--copyright` and the notices in the bundled libraries for the complete licence information.
