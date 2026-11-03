# esp32c6-pico-wifi

See [startup scenarios](../../../../examples/README.md). Run Cargo from this directory to use the local target configuration.

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
