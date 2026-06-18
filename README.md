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
- **image upload** — firmware upload with a tunable chunk size and pipelining (`--fast`), a live progress bar, instantaneous throughput stats, SHA-256 verification, and automatic retry/downgrade
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
| `--scan-secs <N>` | How long to scan when resolving a device (default 15) |
| `--timeout <N>` | Per-operation response timeout in seconds (default 30) |
| `--all-devices` | Scan all BLE devices (the default) |
| `--smp-devices` | Only consider devices advertising the SMP service |
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

# Flash new firmware as fast as the link allows, mark it for test, and reboot
mcumgr-mac -n MySensor image upload firmware.img --fast
mcumgr-mac -n MySensor image test
mcumgr-mac -n MySensor reset
# once it boots the new image and you're happy with it, keep it permanently:
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

By default the scan includes all nearby BLE devices, because many devices
advertise SMP only after connecting (not in their advertisement) and would be
invisible to a filtered scan. Select a specific device with `--name` or `--id`.
Use `--smp-devices` to restrict the scan to devices that advertise the SMP
service.

The cache lives at `~/Library/Application Support/mcumgr-mac/devices.json`.

## Firmware images

`image upload` expects an **MCUboot**-format image (a signed `.img`, e.g. from
`newt create-image` or a Zephyr `zephyr.signed.bin`). Use `image info <file>` to
validate one locally first — it checks the magic, prints the version and size,
and verifies the embedded SHA-256.

Upload writes to the secondary slot; the device will not boot the new image
until you `image confirm` (or `image test`) and `reset`.

## Upload speed and tuning

Firmware upload over BLE is almost always limited by the **Bluetooth link, not
this tool** — and the settings that matter most live in the **device firmware**,
not the host. This section covers the CLI knobs and, more usefully, the firmware
settings that raise the ceiling. mcumgr/SMP runs on several stacks (Apache
**Mynewt/NimBLE**, **Zephyr**, …); the settings below exist in all of them under
different names.

### `image upload` options

| Option | Default | What it does |
| --- | --- | --- |
| `--chunk <N>` | 128 (432 with `--fast`) | Data bytes per chunk. On a DLE link this is the **main lever** (see the model below). Auto-reduced if it exceeds the negotiated MTU. |
| `--window <N>` | 1 (4 with `--fast`) | Keep N chunks in flight (pipelining). Hides per-chunk round-trip latency; the main lever on a **non-DLE** link. |
| `--fast` | off | Preset: a large chunk (432) plus a modest pipeline (window 4). |
| `--erase` | off | Erase the secondary slot before uploading (only needed if the device doesn't erase during upload). |

The uploader **auto-downgrades** anything the device can't handle: an over-large
chunk is stepped down to fit the MTU, a pipeline the device can't keep up with
falls back to sequential, and a busy / bad-state slot triggers an
erase-and-retry. So `--fast` is safe to try anywhere — worst case it quietly
steps down. Redirected/headless runs print an **instantaneous** throughput line
every 5% and an `avg / peak / min` summary at the end, handy for benchmarking
your own tuning.

### A simple model for SMP-over-BLE throughput

Each upload chunk is one SMP request the device acknowledges. A BLE central —
**Apple's especially** (macOS and iOS) — services only a few packets per
*connection event*, so in practice you move roughly **one chunk every one or two
connection events**. That gives a useful rule of thumb:

> **throughput ≈ chunk size ÷ (a small multiple of the connection interval)**

The limit is round-trip cadence, not raw bandwidth, and two consequences follow —
which is why the *right* tuning differs from device to device:

- **Without Data Length Extension (DLE):** a link-layer packet carries only
  ~27 bytes, so any chunk fragments into many packets and you become
  **packet-rate limited**. Chunk size barely matters; **pipelining (`--window`)
  is the lever**, and you plateau around ~8 KB/s.
- **With DLE** (up to 251-byte packets): a chunk rides in one or two packets, so
  the per-chunk round-trip is roughly fixed and **a bigger chunk simply moves
  more bytes per round-trip**. **Chunk size is the lever**; a deep window adds
  little (you're already moving ~one chunk per event) and mainly risks
  overflowing the device's buffers.

Because the bottleneck is the per-event cadence, **2M PHY barely helps on an
Apple central** — it shortens airtime, not the round-trip — and a faster
connection interval helps far more than a faster PHY. (A non-Apple central that
packs many packets per event can do much better than the numbers below.)

### What we measured

On an **nRF52840** peripheral uploading a ~460 KB image to **macOS**, changing one
thing at a time:

| Step | Throughput | Full image |
| --- | --- | --- |
| Original (firmware requested a wide 30–100 ms interval, parked slow) | 0.35 KB/s | ~22 min |
| Fixed **15 ms** interval, slave latency 0 | 3.6 KB/s | ~2 min |
| + **DLE** + `--window` (small chunk) | ~8.5 KB/s | ~55 s |
| + `--chunk 192` | ~11.5 KB/s | ~40 s |
| + **ATT MTU 512** + `--chunk 432` | **~14 KB/s** | ~33 s |

About a **40× improvement**, all from firmware link parameters plus matching the
chunk size to them. Enabling 2M PHY on top added only ~10% (round-trip limited,
not bandwidth limited) and is easy to get wrong on older central hardware, so we
left it off.

### Raising the ceiling (firmware), in order of impact

**1. Connection interval — the biggest single factor.**
A peripheral that requests a *slow or wide* interval (or adds slave latency to
save power) caps throughput hard.

- Apple central hosts **cannot set the interval** — only the peripheral can
  request it, and macOS/iOS only honor requests that follow Apple's *Accessory
  Design Guidelines*: interval a multiple of 15 ms, `min ≥ 15 ms`, small slave
  latency, supervision timeout 2–6 s. (Android exposes
  `requestConnectionPriority()`; Apple has no equivalent.) Aim for **15 ms,
  slave latency 0**, at least during DFU.
- NimBLE: `ble_gap_update_params()` with `itvl_min = itvl_max = BLE_GAP_CONN_ITVL_MS(15)`,
  `latency = 0`; set `BLE_SVC_GAP_PPCP_*` to match. A wide `30–100 ms` range with
  `latency = 1` typically settles near the slow end.
- Zephyr: `bt_conn_le_param_update()`, or `CONFIG_BT_PERIPHERAL_PREF_MIN_INT` /
  `MAX_INT` / `LATENCY`.
- **Verify what was granted** — log the negotiated interval on the
  connection-update event; the request can be silently rejected.

**2. Data Length Extension (DLE) — what makes big chunks pay off.**
Without DLE, link-layer packets carry ~27 bytes, so even a small chunk fragments
into many packets and you're packet-rate limited (the ~8 KB/s plateau, and why
chunk size stops mattering there). With DLE, packets carry up to 251 bytes and a
chunk rides in one or two of them.

- NimBLE: `BLE_LL_CFG_FEAT_DATA_LEN_EXT: 1`, and
  `BLE_LL_SUPP_MAX_TX_BYTES` / `BLE_LL_SUPP_MAX_RX_BYTES: 251`.
- **Gotcha:** enabling the feature isn't always enough. On NimBLE,
  `BLE_LL_CONN_INIT_MAX_TX_BYTES` defaults to 27, which holds the negotiated data
  length down even with DLE on — set it to **251** so the link actually scales up.
- Zephyr: `CONFIG_BT_CTLR_DATA_LENGTH_MAX=251` (plus matching ACL buffer sizes).

**3. ATT MTU and buffers — they cap the chunk.**
With DLE on, your maximum useful chunk is bounded by the negotiated ATT MTU (the
first chunk also carries a SHA-256, so leave headroom). Raise the MTU to allow a
bigger `--chunk`, and make sure the controller has the buffers to back it. NimBLE:
`BLE_ATT_PREFERRED_MTU` (e.g. 512) and a healthy `MSYS_*` mbuf pool; Zephyr:
`CONFIG_BT_L2CAP_TX_MTU` / `CONFIG_BT_BUF_ACL_*`. macOS negotiates up to ~527.

**4. 2M PHY — marginal on Apple centrals, and risky.** NimBLE
`BLE_LL_CFG_FEAT_LE_2M_PHY: 1`; Zephyr `CONFIG_BT_CTLR_PHY_2M=y`. Because uploads
here are round-trip limited, 2M PHY bought only ~10% in our testing. It also
needs the peripheral to actively request the PHY update, and some older central
hardware misbehaves on 2M PHY — test against your targets before enabling it.

**5. Erase-during-upload.** If uploads stall with a bad-state error, the slot may
need erasing first. Mynewt's `IMG_MGMT_LAZY_ERASE: 1` erases sectors on the fly
during upload (so `--erase` is unnecessary); without it — or on other stacks that
don't — use `--erase` (or `image erase`) beforehand. Note that erasing a full,
occupied slot can keep the radio busy long enough to drop the BLE link (and
reboot a device that resets on disconnect); the tool detects this, reconnects,
and confirms the slot is clear, so `--erase` still completes — it just takes a
little longer.

### In short

Tune the firmware link parameters first — a fixed 15 ms interval, then DLE — and
raise the MTU; then set `--chunk` to fill the headroom DLE unlocks (or just use
`--fast`). On a non-DLE device, lean on `--window` instead. Throughput tracks
chunk-size-per-round-trip, so the gains come from the link configuration, not the
host.

## Protocol

SMP messages are an 8-byte header plus a CBOR payload, exchanged over the SMP
GATT service:

- Service UUID: `8d53dc1d-1db7-4cd3-868b-8a527460aa84`
- Characteristic UUID: `da2e7828-fbce-4e01-ae9e-261174997c48`

## License

MIT — see [LICENSE](LICENSE).
