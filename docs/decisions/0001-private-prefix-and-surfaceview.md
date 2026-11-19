# Decision 0001: Private package prefix and Rust terminal surface

**Status:** Accepted for the next implementation phase
**Date:** 2026-11-19

## Decisions

Mossanic will not use the `.moss` package format. The user-facing installation command will be `pkg install <name>`, following the Termux command convention. Installed packages will live in a private Mossanic prefix owned by `org.mossanic`; package installation must not write to Android's system directories or depend on an already-installed Termux app.

Mossanic will keep the Android `SurfaceView` as a minimal framework-owned view hosted by the Activity. Rust owns terminal state, PTY integration, rendering, and the Vulkan surface lifecycle after Kotlin passes the created `Surface` through JNI. The actual `android.view.SurfaceView` class is part of Android's Java framework; the practical Rust-first boundary is therefore a small Kotlin lifecycle/attachment shim, not a Rust-defined subclass.

No new non-Rust Cargo dependency is approved by this decision. The package backend has not yet been selected or implemented. It must expose the `pkg` command and install into the private prefix. Do not present a placeholder command as a working package installer.

## Package compatibility constraint

Termux's `pkg` command is its package-management interface over the APT/dpkg package ecosystem. The Termux execution-environment documentation says packages in its official repositories are compiled for Termux's package name and filesystem layout, and do not work unchanged under a different app package name or prefix. The Termux build guide likewise says that changing the package name or prefix requires rebuilding packages for those values and that packages from different prefixes should not be mixed. Therefore Mossanic must not simply download official Termux `.deb` packages and extract them into an arbitrary app-private directory.

The compatible route is to define Mossanic's app/prefix values in a package build configuration and use a Mossanic-controlled repository/bootstrap built for them. `pkg install` can then retain Termux-style command behavior while packages are built and published for Mossanic. Package signatures, repository trust, dependency resolution, install scripts, rollback, and upgrades are required before this is considered usable.

## Implementation sequence

1. Define a stable private prefix and home directory under the app's private files directory, pass the runtime paths from Kotlin to Rust through JNI, and make the PTY environment derive `HOME`, `PREFIX`, `PATH`, and `TERM` from that configuration.
2. Define the package repository/bootstrap contract for the exact Mossanic app ID `org.mossanic` and target ABI. Decide whether Rust will implement the package client or invoke a bundled, prefix-compatible APT/dpkg toolchain; either way, do not use packages built for `com.termux` unchanged.
3. Add `pkg` command parsing and package operations only after the repository and trusted metadata format are defined. Begin with `pkg update`, `pkg search`, and one small test package, then dependency installation and upgrades.
4. Keep Kotlin responsible only for Activity/View attachment, keyboard/input callbacks, and `SurfaceHolder.Callback`; keep the terminal grid, PTY, and Ash renderer in Rust. Add explicit resize and surface-destroy/join handling before relying on repeated Activity lifecycle events.

## Sources

- [1] Termux execution environment explains that official Termux packages target its app package name and root filesystem layout: https://github.com/termux/termux-packages/wiki/Termux-execution-environment
- [2] Termux package build guide explains custom package-name/prefix builds and repository incompatibility when these values change: https://github.com/termux/termux-packages/wiki/Building-packages
- [3] Termux package-management overview: https://wiki.termux.com/wiki/Package_Management
