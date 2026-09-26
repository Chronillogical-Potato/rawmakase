# Native dependencies

Source builds dynamically link the system `libraw_r`, `lcms2`, C++ runtime and OpenMP runtime. LibRaw is separately licensed under LGPL-2.1/CDDL alternatives; see the installed package and [upstream license files](https://github.com/LibRaw/LibRaw). Little CMS and Rust dependencies retain their own licenses. The project's MIT license applies to RAWmakase's original code, not those libraries. Do not bundle native libraries without their notices and applicable distribution requirements.

Release DMGs and DEB/RPM packages bundle private copies of LibRaw 0.22.2 and Little CMS 2.19.1, with their notices. The native source archives and build script are attached to the GitHub release. Mac apps also bundle their non-system runtime dependencies; Linux packages retain system C++/OpenMP/graphics dependencies. Arch packages continue to use system libraries. See [release packaging](../packaging/RELEASING.md).

Rust dependencies are resolved in Cargo.lock. The UI uses eframe 0.36.1; its compatible egui ecosystem dependencies currently resolve to 0.36.2. Build.rs verifies LibRaw >=0.22 and Little CMS through pkg-config. The tested native versions are LibRaw 0.22.2 and Little CMS 2.19.

On macOS, Apple Clang uses LLVM `libomp` and libc++; Linux keeps GCC OpenMP (`gomp`). The build corrects LibRaw pkg-config entries that still name the obsolete macOS `stdc++` library. Apple Silicon was verified with LibRaw 0.22.0 and Little CMS 2.17.

`src/dng_tone.rs` contains the default ACR3 tone table from Adobe DNG SDK 1.7.1 `dng_render.cpp`, Copyright 2006–2023 Adobe Systems Incorporated. Its license is included in `licenses/Adobe-DNG-SDK.txt` and in local macOS app bundles. No user-supplied Adobe camera or lens profiles are bundled.
