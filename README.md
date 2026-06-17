# mcumgr-mac

A fast, friendly command-line tool for managing **MCUmgr / SMP** devices over
**Bluetooth Low Energy** — upload firmware, inspect and switch images, echo, and
reset. Built in Rust on [btleplug](https://github.com/deviceplug/btleplug)
(CoreBluetooth on macOS).

```
$ mcumgr-mac discover
✓ Found 3 device(s)
╭───┬─────────────────┬──────────────────┬──────────────────────────────────────┬──────╮
│   ┆ Name            ┆ Signal           ┆ Identifier                           ┆ Seen │
╞═══╪═════════════════╪══════════════════╪══════════════════════════════════════╪══════╡
│ ● ┆ MySensor        ┆ ▰▰▰▰  -52 dBm    ┆ 7c0c2145-a228-4579-73dd-523044237149 ┆ ★ 4  │
│ ● ┆ Thermostat      ┆ ▰▰▰▱  -71 dBm    ┆ d4e4d11e-663c-1a61-810b-266603603a1d ┆ —    │
│ ● ┆ (unnamed)       ┆ ▰▰▱▱  -83 dBm    ┆ 5b023bd0-f4a4-5b24-0310-808a098af3fe ┆ —    │
╰───┴─────────────────┴──────────────────┴──────────────────────────────────────┴──────╯
```

## Features

- **discover** — scan and list nearby devices with signal strength and a note of how often you've connected
- **echo** — round-trip a string through the device (a quick connectivity check)
- **reset** — reboot the device
- **image list** — show image slots: version, hash, and flags (active / confirmed / pending / …)
- **image upload** — chunked firmware upload with a live progress bar, SHA-256, and automatic per-chunk retry
- **image test / confirm** — select an image for test-on-next-boot or confirm it permanently
- **image erase** — erase an image slot
- **image info** — parse and validate a local MCUboot image file (no Bluetooth needed)
- **device cache** — remembers devices you connect to and reconnects to your most-used one without you naming it

## Requirements

- **macOS** with Bluetooth. The first run prompts for Bluetooth permission for
  your terminal; grant it (System Settings → Privacy & Security → Bluetooth).
- For building from source: a recent stable **Rust** toolchain (1.74+).

> btleplug is cross-platform, so the tool also builds on Linux (BlueZ) and
> Windows, but it is developed and tested primarily on macOS.

## Installation

### Homebrew (recommended for macOS)

```sh
brew install boogie/tap/mcumgr-mac
```

### Prebuilt binary

Download the latest `mcumgr-mac-macos-universal.tar.gz` from the
[Releases](https://github.com/boogie/mcumgr-mac/releases) page, then:

```sh
tar xzf mcumgr-mac-macos-universal.tar.gz
sudo mv mcumgr-mac /usr/local/bin/
```

Release binaries are a universal build (Intel + Apple Silicon), signed with a
Developer ID and notarized by Apple, so they run without Gatekeeper prompts. If
you ever use an unsigned build, clear the quarantine flag first:
`xattr -d com.apple.quarantine mcumgr-mac`.

### From source with Cargo

```sh
cargo install --git https://github.com/boogie/mcumgr-mac
```

## Building from source

```sh
git clone https://github.com/boogie/mcumgr-mac
cd mcumgr-mac
cargo build --release
# binary at target/release/mcumgr-mac
cargo test          # run the unit tests
```

## Usage

```
mcumgr-mac [OPTIONS] <COMMAND>
```

### Commands

| Command | Description |
| --- | --- |
| `discover` | Scan and list nearby SMP devices |
| `echo <TEXT>` | Echo a string off the device |
| `reset` | Reboot the device |
| `image list` | Show image slots and their state |
| `image upload <FILE>` | Upload a firmware image |
| `image test [HASH]` | Mark an image for test on next boot |
| `image confirm [HASH]` | Confirm an image permanently |
| `image erase [--slot N]` | Erase an image slot |
| `image info <FILE>` | Parse a local MCUboot image (no Bluetooth) |

### Global options

| Option | Description |
| --- | --- |
| `-n, --name <NAME>` | Connect to a device whose advertised name contains `NAME` (case-insensitive) |
| `--id <ID>` | Connect to a specific peripheral id |
| `--scan-secs <N>` | How long to scan when resolving a device (default 5) |
| `--timeout <N>` | Per-operation response timeout in seconds (default 30) |
| `--all-devices` | Scan all BLE devices, not just those advertising the SMP service |
| `--no-cache` | Do not read or update the device cache this run |
| `-v, --verbose` | Verbose diagnostic logging |

### Examples

```sh
# See what's nearby
mcumgr-mac discover

# Talk to your usual device (most-frequently connected) — no name needed
mcumgr-mac echo "hello"

# Target a device by name
mcumgr-mac -n MySensor image list

# Flash new firmware, then make it permanent
mcumgr-mac -n MySensor image upload firmware.img
mcumgr-mac -n MySensor reset
mcumgr-mac -n MySensor image confirm

# Inspect a firmware file without any device
mcumgr-mac image info firmware.img
```

## How device selection works

For every command that needs a connection, the tool:

1. Loads the device cache and ranks known devices by connection count (ties
   broken by recency), filtered by `--name` if given.
2. Starts a BLE scan and **connects the instant the preferred device appears** —
   your named device, or your most-used cached device.
3. If nothing preferred shows up within `--scan-secs`, it falls back to the
   best-ranked cached device seen, or errors rather than connecting to an
   arbitrary device.
4. On a successful connection, the device is recorded/updated in the cache.

By default the scan is filtered to the SMP service UUID. Many devices advertise
SMP only after connecting (not in their advertisement), so if your device does
not appear in `discover`, add `--all-devices` and select it by `--name` or
`--id`.

The cache lives at `~/Library/Application Support/mcumgr-mac/devices.json`.

## Firmware images

`image upload` expects an **MCUboot**-format image (a signed `.img`, e.g. from
`newt create-image` or a Zephyr `zephyr.signed.bin`). Use `image info <file>` to
validate one locally first — it checks the magic, prints the version and size,
and verifies the embedded SHA-256.

Upload writes to the secondary slot; the device will not boot the new image
until you `image confirm` (or `image test`) and `reset`.

## Protocol

SMP messages are an 8-byte header plus a CBOR payload, exchanged over the SMP
GATT service:

- Service UUID: `8d53dc1d-1db7-4cd3-868b-8a527460aa84`
- Characteristic UUID: `da2e7828-fbce-4e01-ae9e-261174997c48`

## License

MIT — see [LICENSE](LICENSE).
