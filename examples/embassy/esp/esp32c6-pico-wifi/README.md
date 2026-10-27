# Waveshare ESP32-C6-Pico command console

One firmware application uses stable Rust with `riscv32imac-unknown-none-elf`,
ESP HAL/RTOS, Wi-Fi DHCP, and runtime-selected Zenoh commands. No Xtensa compiler
is needed.

From the repository root:

```sh
nix develop
export WIFI_SSID=your-access-point
export WIFI_PASSWORD=your-password
export ENDPOINT=tcp/192.168.1.10:7447  # Optional default; z_open also accepts an address
just build-esp32c6
just esp32c6
```

The default console is UART1: TX GPIO4, RX GPIO5, 115200 baud, 8N1, no flow
control. Use a 3.3 V USB-to-UART adapter with TX/RX crossed and GND connected.
The board's native USB port is a different interface. To use that cable:

```sh
just esp32c6 console-usb
just terminal /dev/cu.usbmodemXXXX
# For broker capability in the same application:
just esp32c6 alloc,console-usb
```

USB mode suppresses defmt output on the CLI stream. Select the board for flashing
with `ESPFLASH_PORT` if multiple serial devices are connected. The flash recipe
returns without opening a defmt monitor; the text terminal is a separate command.

From this directory, equivalent build/flash commands are
`cargo build --locked --release --bin zenoh-app` and
`cargo run --locked --release --bin zenoh-app --features console-usb`.
Use `help`, `status`, `z_open <endpoint>`, then commands such as
`z_get demo/example/**`. Ctrl-C stops the foreground operation without reflashing.

The radio still needs a 73,744-byte heap when Zenoh allocation is disabled.
This C6 application places it in ordinary SRAM because reclaimed RAM is smaller.
`alloc` selects allocated Zenoh storage and broker capability; it does not create
a second binary. Shared ESP startup and runtime code live in `examples/embassy/esp/`.

See [command syntax and limits](../../../../docs/applications.md#cli) and the
[hardware smoke test](../../../../docs/applications.md#hardware-smoke-test). Board pinout and recovery:
[Waveshare ESP32-C6-Pico](https://www.waveshare.com/wiki/ESP32-C6-Pico).

## Startup scenarios without a CLI

The `cli` feature remains enabled by default. To build without a command console,
run `COUNT=3 just build-esp32c6-headless pub` from the repository root.
Use `just esp32c6-headless pub` to build and flash; scenario settings are
compiled into the firmware. Add `alloc` after the scenario to select allocated
storage. See the [headless guide](../../../../docs/applications.md#headless).

## Scoped-session hardware test

This optional startup test uses local Zenoh configuration, transport, session
and callback state. The session driver and get operation are polled in one task
with `select`; it does not use `zenoh::session!`, static Zenoh cells or
`Box::leak`. HAL/network tasks still own their required platform storage.

The firmware opens eight connections against one host queryable, alternating four
successful gets with four cancelled operations. Reopening beyond the link
manager's three slots checks that dropping each scope returns its slot. The
final success message is emitted after the local configuration is dropped.

Allow about two minutes per run. The test waits 11 seconds between connections
to let the router retire the previous ten-second lease. Dropping a local session
releases MCU storage but does not send a Zenoh Close; this test does not establish
that immediate reconnects with the same identity work.

Start a Zenoh router reachable from both the host and board, then run the host
peer from the repository root (set `ENDPOINT` to that router):

```sh
SCENARIO=queryable KEY_EXPR=test/scoped-session VALUE=scoped-reply just std-headless
```

With local `WIFI_SSID`, `WIFI_PASSWORD` and `ENDPOINT` set, flash and monitor:

```sh
just esp32c6-scoped-session
# Repeat with heap-backed Zenoh storage:
just esp32c6-scoped-session alloc
```

The flash runner prints the application path; use it with
`espflash monitor --chip esp32c6 --elf <path> --port <port>` to decode defmt logs.
Expect eight cycle messages and
`SCOPED SESSION PASS: 8 cycles; local config dropped`.
Any failure produces `SCOPED SESSION FAIL` and stops the test.

For release-link verification with placeholder settings and no flashing, use
`just build-esp32c6-scoped-session` and its `alloc` variant. Enabling
`scoped-session-test` selects this test instead of the normal startup scenario;
it requires a build with default features disabled.
