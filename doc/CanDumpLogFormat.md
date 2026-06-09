# CAN Dump Log Line Format

This is an AI-generated reference for the text log format emitted by [candump](https://manpages.debian.org/testing/can-utils/candump.1.en.html), as parsed by the `socketcan::dump` module (`src/dump.rs`).

## Scope and Source of Truth

This document describes the **log format** produced with the `candump -L` command; the format that [canplayer](https://manpages.debian.org/testing/can-utils/canplayer.1.en.html) can read back and that this crate parses and emits. It does not cover candump's other human-readable output modes, only the log format.

There is no formal specification. The closest thing to an authoritative grammar is the `parse_canframe()` documentation comment in can-utils [`lib.h`](https://github.com/linux-can/can-utils/blob/master/lib.h), and the format is ultimately *defined* by the `lib.c` / `candump.c` source. This document reproduces that grammar and notes where this crate's parser is stricter.

## Line structure

Each line is three space-separated fields:

```text
(<sec>.<usec>) <iface> <frame>
```

Example lines:

```text
(1735270496.916858) can0 110#00112233
(1735270588.936508) can0 120##500112233445566778899AABB
(1735279041.257318) can1 104#R
(1735279048.349278) can1 110#R4
(1469439874.299654) can1 104#
```

## Grammar (ABNF-style)

```abnf
record     = "(" sec "." usec ")" SP iface SP frame

sec        = 1*DIGIT              ; whole seconds since the epoch
usec       = 6DIGIT               ; microseconds, always exactly 6 digits
iface      = 1*VCHAR              ; interface name, e.g. "can0", "vcan1"

frame      = can-id ( classical / canfd )

can-id     = 3HEXDIG              ; SFF — standard 11-bit identifier
           / 8HEXDIG              ; EFF — extended 29-bit identifier, OR an error frame

classical  = "#" ( rtr / data ) [ "_" dlc8 ]
canfd      = "##" flags data

rtr        = "R" [ HEXDIG ]       ; remote frame; optional single-nibble DLC (0..F), absent = 0
data       = *( 2HEXDIG )         ; payload bytes; 0..8 for classical, 0..64 for CAN FD
flags      = HEXDIG               ; single nibble mapped onto canfd_frame.flags
dlc8       = HEXDIG               ; "len8 DLC" escape; only meaningful when data is 8 bytes

HEXDIG     = DIGIT / "A".."F" / "a".."f"
```

## Fields

### Timestamp — `(<sec>.<usec>)`

Absolute time, wrapped in parentheses, as a floating-point `time_t` value, as seconds from the UNIX Epoch. `candump` always emits exactly **six** fractional digits (microsecond resolution). This crate's parser requires exactly six digits and rejects any other count rather than guessing the precision.

### Interface — `<iface>`

The CAN interface name the frame was captured on (e.g. `can0`, `vcan1`, `slcan0`). Up to `IFNAMSIZ - 1` characters.

### Frame — `<can-id><body>`

A CAN identifier in hexadecimal, immediately followed by a body whose leading character(s) select the frame kind.

## Identifier width

The number of hex digits in `can-id` distinguishes the frame format:

- **3 digits** — Standard Frame Format (SFF), an 11-bit identifier (`0x000`..`0x7FF`).
- **8 digits** — Extended Frame Format (EFF), a 29-bit identifier (`0x00000000`..`0x1FFFFFFF`), **or** an error frame (see below).

There is no syntactic difference between an extended data/remote frame and an error frame; they are distinguished only by the `CAN_ERR_FLAG` bit in the parsed numeric identifier.

## Frame body forms

### Classical data frame — `#<data>`

A `#` followed by an even number of hex digits, two per data byte. Zero bytes (`123#`) is valid and means an empty payload. Maximum 8 bytes for classical CAN.

```text
123#              SFF, 0 data bytes
123#1122334455667788   SFF, 8 data bytes
12345678#DEADBEEF      EFF, 4 data bytes
```

### Remote frame (RTR) — `#R[<dlc>]`

A `#R`, optionally followed by a single hex digit giving the requested DLC (`0`..`F`). An absent digit means DLC 0. Remote frames carry no data — only the DLC.

```text
104#R             RTR, DLC 0
110#R4            RTR, DLC 4
```

CAN FD has no remote-frame concept; RTR applies to classical frames only.

### CAN FD frame — `##<flags><data>`

A double `##` distinguishes CAN FD from classical CAN. The first character after `##` is a single hex nibble carrying the FD flags; the remainder is the payload (even hex digits, up to 64 bytes).

```text
120##500112233445566778899AABB    FD, flags=5, 12 data bytes
080##0                            FD, flags=0, 0 data bytes
```

#### Flags nibble

The nibble maps directly onto the kernel `canfd_frame.flags` field:

| Bit  | Name        | Meaning                        |
|------|-------------|--------------------------------|
| 0x1  | `CANFD_BRS` | Bit Rate Switch                |
| 0x2  | `CANFD_ESI` | Error State Indicator          |
| 0x4  | `CANFD_FDF` | FD Frame (CAN FD, not classic) |

So `##1…` is BRS, `##5…` is BRS|FDF, etc.

### Error frame — `<8 hex>#<data>`

An error frame is an 8-hex-digit identifier with the `CAN_ERR_FLAG` bit (`0x20000000`) set, followed by `#` and the 8 error-class data bytes. It is syntactically identical to an extended data frame; the `CAN_ERR_FLAG` bit in the parsed identifier is what marks it as an error frame. Error frames never carry the RTR bit.

```text
20000004#0000000000000000    error frame, CAN_ERR_FLAG set, error class 0x4
```

### len8 DLC escape — `#<8 data bytes>_<dlc>`

Classical CAN can encode a raw DLC value greater than 8 while still carrying only 8 data bytes. candump represents this by appending `_` and a single hex DLC nibble after exactly 8 data bytes. The suffix is only meaningful (and only emitted) when the frame has 8 data bytes and a raw DLC of `9`..`F`.

```text
123#1122334455667788_E    8 data bytes, raw DLC = 0xE (14)
```

## Examples

| Line body          | Meaning                                              |
|--------------------|------------------------------------------------------|
| `123#`             | SFF, empty payload                                   |
| `123#1122334455667788` | SFF, 8 data bytes                                |
| `123#R`            | SFF remote frame, DLC 0                              |
| `123#R7`           | SFF remote frame, DLC 7                              |
| `123#1122334455667788_E` | SFF, 8 data bytes, raw DLC = 14                |
| `12345678#DEADBEEF`| EFF, 4 data bytes                                    |
| `123##0112233`     | CAN FD, flags=0, 3 data bytes                        |
| `123##5112233`     | CAN FD, flags=BRS\|FDF, 3 data bytes                 |
| `20000004#0000000000000000` | error frame, error class 0x4                |

## Notes on this crate's parser

The reader in `src/dump.rs` follows the grammar above, with these deliberate choices:

- The microsecond field must be **exactly six digits**; other lengths are rejected (`ParseError::InvalidTimestamp`). candump always emits six, so this is stricter than the permissive C parser but correct for real candump output.
- Each line is read through a 64 KiB cap so a corrupt log cannot exhaust memory; an over-long line yields `ParseError::InvalidCanFrame`.
- Remote-frame DLC parse failures are surfaced as errors rather than silently treated as 0; an empty DLC still parses as 0.
