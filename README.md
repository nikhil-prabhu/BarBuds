<div align="center">
  <img src="assets/logo.svg" alt="BarBuds logo: two clinking beer mugs" width="128">
  <h1>BarBuds</h1>
  <img alt="maintenance-status" src="https://img.shields.io/badge/maintenance-experimental-blue.svg">
</div>

A native macOS menu bar utility for configuring wireless earbuds: battery levels, noise control, EQ and device settings, without the vendor's phone app.

BarBuds lives in the menu bar and automatically lists every supported pair of earbuds connected to your Mac. There's nothing to set up: pair your earbuds in System Settings › Bluetooth as usual, then click the menu bar icon.

## Supported earbuds

| Earbuds | Battery (left, right, case) | Noise control | EQ | Settings |
|---|---|---|---|---|
| Google Pixel Buds Pro (1st gen) | ✓ | Off, Noise Cancelling, Transparency | 5-band (±6 dB) with presets | Volume EQ, In-Ear Detection |
| Google Pixel Buds A-Series | ✓ | – | Bass level (−1 to +4) | Adaptive Sound, Volume EQ, In-Ear Detection |

Settings changed elsewhere, such as from your phone or with a gesture on the earbuds, update in BarBuds as the earbuds report them. Pixel Buds Pro only report a noise-control mode while they're being worn, so noise control is unavailable while they're in the case (when in-ear detection is turned on).

More models are planned. See [Adding a device](#adding-a-device).

## Requirements

- macOS 27 or later
- Earbuds paired with the Mac over Bluetooth. BarBuds asks for Bluetooth access the first time it runs.

There are no prebuilt releases yet. Signed builds via GitHub Releases and Homebrew are planned. For now, build from source.

## Building from source

You'll need Xcode 27, a Rust toolchain ([rustup](https://rustup.rs)), and these one-time extras:

```sh
cargo install cargo-swift@0.11.1 --locked
rustup target add x86_64-apple-darwin   # the Swift package is a universal (arm64 + x86_64) binary
```

Then:

```sh
make core                                  # build the Rust core into the BarBudsCore Swift package
open app/BarBuds/BarBuds.xcodeproj         # then Product › Run
```

Run `make core` again whenever the Rust code changes. Run the core's tests with `cd core && cargo test`.

Debug builds add a **Debug** section to the popover's ⋯ menu that simulates earbuds connecting and disconnecting, so you can work on the UI without hardware.

## How it works

BarBuds is split into two halves:

- **`core/`**: a Rust library that knows each earbud protocol. It's [sans-I/O](https://sans-io.readthedocs.io): it never touches Bluetooth. It only turns the bytes the earbuds send into battery and settings state, and settings changes into the bytes to send back. Each real protocol is unit-tested against frames captured from real earbuds. The core is exposed to Swift with [UniFFI](https://mozilla.github.io/uniffi-rs/) and packaged by [cargo-swift](https://github.com/antoniusnaumann/cargo-swift).
- **`app/`**: a thin SwiftUI app. It finds connected earbuds and opens their control channel over Classic Bluetooth RFCOMM with IOBluetooth. It passes bytes between the channel and the core and draws the menu bar popover.

| Earbuds | Protocol |
|---|---|
| Pixel Buds Pro | Google's "Maestro": [pigweed pw_rpc](https://pigweed.dev/pw_rpc/) protobuf calls in HDLC frames |
| Pixel Buds A-Series | A simpler control channel: protobuf messages behind a 4-byte header, with settings stored as numbered values |

## Adding a device

Each model is a driver under `core/src/drivers/` that implements one internal trait: describe the device's capabilities, decode incoming frames, and encode settings changes. The app needs no changes for a new model. It finds devices by the RFCOMM service each driver advertises and draws only the controls a model supports.

If the protocol isn't documented anywhere, you can capture it from the vendor's Android app:
1. On the phone, turn on **Developer options › Enable Bluetooth HCI snoop log**, choosing *Enabled* (not *Filtered*). Turn Bluetooth off and on again so logging starts.
2. Change one setting at a time in the vendor app, a few seconds apart, and note the order.
3. Take a bug report (`adb bugreport`) and extract `FS/data/misc/bluetooth/logs/btsnoop_hci.log`. Delete the rest of the report, because it contains a lot of personal data.

Please leave device identifiers, such as serial numbers, out of any frames you commit as test fixtures.

## Credits

- **[pbpctrl](https://github.com/qzed/pbpctrl)** by Maximilian Luz: a Linux tool for controlling Pixel Buds Pro. BarBuds' Pixel Buds Pro support is built on pbpctrl's reverse-engineering of the Maestro protocol (RPC channels, message definitions and setting IDs), and its HDLC test vectors are part of BarBuds' tests. pbpctrl is licensed under MIT / Apache-2.0.
- The Pixel Buds A-Series protocol was reverse-engineered for BarBuds from Bluetooth captures of the official app.
- The logo is the 🍻 "clinking beer mugs" emoji from Google's [Noto Emoji](https://github.com/googlefonts/noto-emoji), licensed under Apache-2.0.

BarBuds is not affiliated with or endorsed by Google or any other earbud manufacturer.

## License

[GPL-3.0](LICENSE)
