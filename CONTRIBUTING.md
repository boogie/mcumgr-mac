# Contributing to mcumgr-mac

Thank you for your interest in contributing to mcumgr-mac! This document covers
how the project is laid out, how to build and test it, and how to submit
changes.

## Table of Contents

- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Code Structure](#code-structure)
- [Testing](#testing)
- [Coding Standards](#coding-standards)
- [Submitting Changes](#submitting-changes)
- [Adding New Features](#adding-new-features)
- [Reporting Bugs](#reporting-bugs)
- [License](#license)
- [Questions](#questions)

## Getting Started

mcumgr-mac is a command-line tool for managing **MCUmgr / SMP** devices over
**Bluetooth Low Energy** — uploading firmware, inspecting and switching images,
echo, and reset. It is written in Rust on top of
[btleplug](https://github.com/deviceplug/btleplug) (CoreBluetooth on macOS).

Before contributing, please:

1. Read the [README.md](README.md) for the project overview and the upload
   speed/tuning notes.
2. Get familiar with SMP: messages are an 8-byte header plus a CBOR payload over
   the SMP GATT service. The "Protocol" section of the README has the UUIDs.
3. Check existing issues and pull requests to avoid duplicating work.

## Development Setup

### Prerequisites

- **macOS** with Bluetooth. The tool is developed and tested on macOS; btleplug
  is cross-platform, so it also builds on Linux (BlueZ) and Windows, but those
  are not regularly exercised.
- A recent stable **Rust** toolchain (1.74+) — install via
  [rustup](https://rustup.rs).
- A Bluetooth LE device running an MCUmgr/SMP server (Mynewt/NimBLE or Zephyr)
  with an MCUboot bootloader, for testing anything that touches a device.
- Optional: an MCUboot-format firmware image (e.g. from `newt create-image` or a
  Zephyr `*.signed.bin`) for upload testing.

### Build and run

```bash
git clone https://github.com/boogie/mcumgr-mac.git
cd mcumgr-mac

cargo build                 # debug build
cargo run -- discover       # run a subcommand
cargo build --release       # optimized binary at target/release/mcumgr-mac
```

On first run, macOS prompts for Bluetooth permission for your terminal — grant
it (System Settings → Privacy & Security → Bluetooth).

### Useful commands

```bash
cargo run -- -v <command>   # verbose diagnostic logging
cargo run -- image info firmware.img   # parse/validate an image locally (no BLE)
```

`image info` is handy while developing the image parser because it needs no
device.

## Code Structure

The crate is split into a **library** (`mcumgr_mac`, protocol and format logic
that needs no Bluetooth and is unit-tested) and a **binary** (`mcumgr-mac`, the
CLI, BLE transport, and terminal UI).

```
mcumgr-mac/
├── src/
│   ├── main.rs          # Binary entry point: arg intercept, custom help, logging
│   ├── cli.rs           # clap command-line definitions
│   ├── help.rs          # Hand-rendered, grouped --help output
│   ├── transport.rs     # BLE scan/resolve + SmpSession over GATT (btleplug)
│   ├── ui.rs            # Terminal UI: spinners, tables, progress bars
│   ├── commands/
│   │   ├── mod.rs       # Dispatch + shared "resolve device and connect" helper
│   │   ├── discover.rs  # `discover`
│   │   ├── image.rs     # `image` subcommands (upload/list/test/confirm/erase/info)
│   │   └── os.rs        # `echo`, `reset`
│   ├── lib.rs           # Library crate root
│   ├── cache.rs         # Device cache (reconnect to your most-used device)
│   ├── error.rs         # Error types
│   ├── image.rs         # MCUboot image parsing/validation
│   └── smp/
│       ├── mod.rs       # SMP framing (8-byte header + CBOR assembly)
│       ├── groups.rs    # Management group / command IDs
│       └── messages.rs  # Request/response payloads
├── Cargo.toml
├── README.md
├── CONTRIBUTING.md      # This file
└── LICENSE
```

### Key modules

- **`transport.rs`** — `resolve_peripheral` (cache-biased scan and connect) and
  `SmpSession` (send/receive SMP frames, windowed/pipelined helpers). This is the
  only place that talks to btleplug.
- **`commands/image.rs`** — the upload engine: chunking, the `--fast` preset,
  windowed pipelining, and the auto-downgrade/erase-and-retry logic.
- **`smp/`** and **`image.rs`** — pure protocol/format code with no I/O; this is
  where most unit tests live.

## Testing

CI (`.github/workflows/ci.yml`) runs three checks on every push and pull request,
and they must all pass:

```bash
cargo fmt --all --check               # formatting
cargo clippy --all-targets -- -D warnings   # lints (warnings are errors)
cargo test --all                      # unit tests
```

Please run these locally before submitting. Use `cargo fmt` (without `--check`)
to apply formatting.

### Unit tests

Protocol and format logic (SMP framing, CBOR payloads, MCUboot image parsing,
the device cache) is unit-tested and runs without any hardware. New logic in
those areas should come with tests.

### Manual testing with a device

Hardware-dependent behavior can't be unit-tested, so verify on a real device when
you change anything that touches BLE or the upload flow:

- [ ] `discover` lists nearby devices with signal strength
- [ ] Connect by `--name`, by `--id`, and via the cache (no name given)
- [ ] `image list` shows slots, versions, flags, and hashes
- [ ] `image upload firmware.img` completes, with progress and end-of-upload stats
- [ ] `image upload --fast` works and is faster
- [ ] `image upload --erase` erases and re-uploads
- [ ] `image test` / `image confirm` / `reset` behave as expected
- [ ] `echo "hello"` round-trips
- [ ] Errors (device out of range, bad image, busy slot) produce clear messages

## Coding Standards

- **Formatting:** `cargo fmt` (default rustfmt). No manual deviations.
- **Lints:** keep `cargo clippy --all-targets -- -D warnings` clean.
- **Naming:** standard Rust — `snake_case` for functions/variables, `PascalCase`
  for types, `SCREAMING_SNAKE_CASE` for constants.
- **Errors:** the binary uses [`anyhow`](https://docs.rs/anyhow) with
  `.context(...)` for user-facing messages; the library defines typed errors with
  [`thiserror`](https://docs.rs/thiserror). Prefer adding context over swallowing
  errors.
- **Async:** the tool is built on `tokio`; keep BLE/I/O on async paths and avoid
  blocking the runtime.
- **Separation:** keep protocol/format logic in the library (no I/O, so it stays
  testable); keep Bluetooth, CLI, and terminal output in the binary.
- **Comments:** match the density and style of the surrounding code. Explain
  *why* for non-obvious BLE/protocol behavior; document public library APIs with
  doc comments.

Write code that reads like the code already around it.

## Submitting Changes

1. **Fork** the repository and create a feature branch:
   ```bash
   git checkout -b feature/your-feature-name
   ```
2. **Make your changes**, following the coding standards and adding tests for new
   library logic.
3. **Run the checks** locally (`cargo fmt --all --check`, `cargo clippy
   --all-targets -- -D warnings`, `cargo test --all`).
4. **Commit** with clear, descriptive messages, e.g.:
   - `image upload: add --window override`
   - `transport: fix name-population race on connect`
   - `docs: document the DLE tuning gotcha`
5. **Push** your branch and open a **Pull Request**, describing what it does, how
   you tested it (which device/firmware, if applicable), and any breaking
   changes.

### Pull Request checklist

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` is clean
- [ ] `cargo test --all` passes
- [ ] New library logic has unit tests
- [ ] README updated if behavior or options changed
- [ ] Device-affecting changes were tested on real hardware
- [ ] No stray debug prints or commented-out code

## Adding New Features

### Adding a new SMP command

1. Add the group/command IDs in `src/smp/groups.rs` (if new).
2. Define the request/response payloads in `src/smp/messages.rs`.
3. Add a handler — typically a function in the relevant `src/commands/*.rs` that
   builds the request, sends it via `SmpSession`, and decodes the response.
4. Wire it into the CLI in `src/cli.rs` and dispatch in `src/commands/mod.rs`.
5. Add unit tests for the payload encoding/decoding, and test the command on a
   device.

### Adding a new CLI subcommand

1. Add the variant in `src/cli.rs` (with clap attributes; put command-specific
   flags under the appropriate `help_heading`).
2. Mirror it in `src/help.rs` so the hand-rendered help stays in sync.
3. Implement and dispatch it in `src/commands/`.

## Reporting Bugs

Before filing, please search existing issues and try the latest `main`. A good
report includes:

- **What you expected** vs. **what happened**
- **Steps to reproduce** (the exact command line)
- **Output** with `-v` (verbose), if relevant
- **Environment:** macOS version, Rust version (`rustc --version`), and the
  device/firmware (e.g. nRF52840 with Mynewt/NimBLE)

## License

By contributing, you agree that your contributions will be licensed under the
project's [MIT License](LICENSE).

## Questions?

If something here is unclear, open an issue with your question — improvements to
this guide and the docs are welcome too.
