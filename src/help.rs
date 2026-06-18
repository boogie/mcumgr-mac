//! Hand-rolled help output.
//!
//! clap cannot group options across subcommands (its `flatten_help` renders one
//! section per subcommand with a `-h` line and the option-less commands too), so
//! we print a single, consistent, grouped help for any `-h`/`--help` at any
//! level, and for no arguments.

/// Print the full grouped help.
pub fn print() {
    print!(
        "\
mcumgr-mac {version}
A friendly CLI for managing MCUmgr / SMP devices over Bluetooth Low Energy.

Usage: mcumgr-mac [OPTIONS] <COMMAND>

Commands:
  discover              Scan and list nearby SMP devices
  echo <TEXT>           Echo a string off the device
  reset                 Reboot the device
  image list            Show image slots and their state
  image upload <FILE>   Upload a firmware image
  image test [HASH]     Mark an image for test on the next boot
  image confirm [HASH]  Confirm an image permanently
  image erase           Erase an image slot
  image info <FILE>     Parse a local MCUboot image file (no Bluetooth)

Options:
  -n, --name <NAME>     Connect to a device whose advertised name contains NAME
      --id <ID>         Connect to a specific peripheral id
      --scan-secs <N>   Seconds to scan when resolving a device [default: 15]
      --timeout <N>     Per-operation response timeout, in seconds [default: 30]
      --all-devices     Scan all BLE devices (the default)
      --smp-devices     Only consider devices advertising the SMP service
      --no-cache        Do not read or update the device cache this run
  -v, --verbose         Verbose diagnostic logging
  -h, --help            Print help
  -V, --version         Print version

Options (image upload specific):
      --slot <N>        Target slot [default: 0]
      --chunk <N>       Data bytes per chunk [default: 128, or 432 with --fast];
                        bigger is faster on a DLE link but is MTU-limited
      --window <N>      Chunks kept in flight, pipelined [default: 1, or 4 with --fast]
      --erase           Erase the secondary slot before uploading
      --fast            Fastest settings (large chunk + pipelining), auto-downgrading

Options (image test/confirm specific):
      [HASH]            Image hash as hex; if omitted, the relevant image is chosen

Options (image erase specific):
      --slot <N>        Slot to erase [default: 1]
",
        version = env!("CARGO_PKG_VERSION")
    );
}
