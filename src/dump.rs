// socketcan/src/dump.rs
//
// Implements candump format parsing.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! candump format parsing
//!
//! Parses the text log format emitted by `candump -L`, which is part of
//! [can-utils](https://github.com/linux-can/can-utils).
//!
//! Can be parsed by a [`Reader`] object. The API is inspired by the
//! [csv](https://crates.io/crates/csv) crate. [`CanDumpRecord`] implements
//! `Display`, emitting the same format, so records read from a log
//! round-trip back to identical text.
//!
//! Example:
//!
//! ```text
//! (1735270496.916858) can0 110#00112233
//! (1735270509.245511) can0 110#44556677
//! (1735270588.936508) can0 120##500112233445566778899AABB
//! (1735270606.171980) can0 122##500112233445566778899AABBCC000000
//! (1735279041.257318) can1 104#R
//! (1735279048.349278) can1 110#R4
//! (1469439874.299654) can1 104#
//! (1785099856.242430) can1 20000004#000C000000000000
//! ```
//!
//! # Grammar
//!
//! The authoritative definition is the `parse_canframe()` doc comment in
//! can-utils `lib.h`, with the sending side implemented in `lib.c`. A fuller
//! treatment, including worked examples and the exact set of deliberate
//! deviations this parser makes, is in `doc/CanDumpLogFormat.md` in the
//! crate repository.
//!
//! ```text
//! record    = "(" sec "." usec ")" SP iface SP frame
//! usec      = 6DIGIT           ; exactly six digits
//! frame     = can_id ( classical / canfd )
//! can_id    = 3HEXDIG          ; SFF — standard 11-bit identifier
//!           | 8HEXDIG          ; EFF — extended 29-bit identifier,
//!                              ;       OR an error frame (see below)
//! classical = "#" ( rtr / data ) [ "_" dlc8 ]
//! canfd     = "##" flags data
//! rtr       = "R" [ HEXDIG ]   ; remote frame; optional DLC nibble, 0..8 usable
//! data      = *(2HEXDIG)       ; 0..8 bytes classical, 0..64 bytes FD
//! flags     = HEXDIG           ; canfd_frame.flags: BRS=0x1 ESI=0x2 FDF=0x4
//! dlc8      = HEXDIG           ; "len8 DLC" escape; see the caveat below
//! ```
//!
//! # The identifier's width picks the format
//!
//! Three hex digits means a standard (SFF) identifier and eight means an
//! extended (EFF) one — the *width* decides, never the numeric value. The two
//! spellings are not nested, so both of these are meaningful and distinct:
//!
//! ```text
//! 123#01        standard frame, ID 0x123
//! 00000123#01   extended frame, ID 0x123
//! ```
//!
//! A value that does not fit the width it was written in — `800#01`, which is
//! not a legal 11-bit identifier — is rejected rather than reinterpreted as
//! the other format. Any other width is rejected too; candump emits only
//! these two.
//!
//! # The eight-digit identifier is ambiguous
//!
//! An eight-digit identifier is **either** an extended (29-bit) data or
//! remote frame **or** an error frame. There is no syntactic difference
//! between the two; they are distinguished solely by the `CAN_ERR_FLAG` bit
//! (`0x2000_0000`) in the parsed numeric value:
//!
//! ```text
//! 1FFFFFFF#0102               extended data frame, ID 0x1FFFFFFF
//! 20000004#000C000000000000   error frame, error class CAN_ERR_CRTL
//! ```
//!
//! An error frame's payload is the eight error-class bytes rather than bus
//! data; see the [errors module](crate::errors) for their layout. Decode
//! them with [`CanErrorFrame::into_error()`], remembering that one frame
//! usually reports several causes at once. Error frames are always
//! classical, so neither the FD nor the remote form applies to them.
//!
//! # Unsupported: the `_dlc` suffix
//!
//! Classical CAN can carry a raw DLC above 8 while still holding only eight
//! data bytes, which candump writes as a `_` and one hex nibble after the
//! payload — `123#1122334455667788_E`. **This parser does not yet accept
//! that form**; such a line is rejected with
//! [`ParseError::InvalidCanFrame`]. Supporting it needs somewhere to put
//! the raw DLC, which [`CanDataFrame`] does not currently have.

use crate::{
    CanAnyFrame, CanDataFrame, CanErrorFrame, CanFdFrame, CanFrame, CanId, CanRemoteFrame,
    ConstructionError,
    id::{CAN_ERR_FLAG, CAN_ERR_MASK, FdFlags},
};
use embedded_can::{Frame as EmbeddedFrame, Id};
use hex::FromHex;
use libc::canid_t;
use std::{
    fmt,
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
};
use thiserror::Error;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// candump line parse error
#[derive(Error, Debug)]
#[cfg_attr(feature = "serde", derive(Deserialize), serde(from = "ParseErrorRepr"))]
pub enum ParseError {
    /// I/O Error
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Unexpected end of line
    #[error("Unexpected end of line")]
    UnexpectedEndOfLine,
    /// Invalid time stamp
    #[error("Invalid timestamp")]
    InvalidTimestamp,
    /// Invalid device name
    #[error("Invalid device name")]
    InvalidDeviceName,
    /// Invalid CAN frame
    #[error("Invalid CAN frame")]
    InvalidCanFrame,
    /// Error creating the frame
    #[error(transparent)]
    ConstructionError(#[from] ConstructionError),
}

/// Serialized form of a [`ParseError`].
///
/// Mirrors the treatment of the crate-level [`Error`](crate::Error): the
/// `Io` variant cannot round-trip faithfully because `io::Error` implements
/// neither serde trait, so it is reduced to a kind name and a message. Every
/// other variant round-trips exactly.
#[cfg(feature = "serde")]
#[derive(Debug, Serialize, Deserialize)]
pub enum ParseErrorRepr {
    /// An I/O error, reduced to a kind name and a message.
    ///
    /// Lossy in the same ways as [`crate::errors::ErrorRepr::Io`]:
    /// `raw_os_error()` is `None` afterwards and an unrecognised kind name
    /// becomes `io::ErrorKind::Other`.
    Io {
        /// The [`io::ErrorKind`], by name
        kind: String,
        /// The original error's `Display` text
        message: String,
    },
    /// Unexpected end of line
    UnexpectedEndOfLine,
    /// Invalid time stamp
    InvalidTimestamp,
    /// Invalid device name
    InvalidDeviceName,
    /// Invalid CAN frame
    InvalidCanFrame,
    /// Error creating the frame
    ConstructionError(ConstructionError),
}

/// Borrows an error to build its serialized form.
///
/// Taken by reference because [`ParseError`] is not `Clone` — `io::Error` is
/// not — so the crate-level [`Error`](crate::Error) can reach this from its
/// own `Serialize` impl without owning the error.
#[cfg(feature = "serde")]
impl From<&ParseError> for ParseErrorRepr {
    fn from(err: &ParseError) -> Self {
        use crate::errors::io_kind_name;
        match err {
            ParseError::Io(e) => Self::Io {
                kind: io_kind_name(e.kind()).to_string(),
                message: e.to_string(),
            },
            ParseError::UnexpectedEndOfLine => Self::UnexpectedEndOfLine,
            ParseError::InvalidTimestamp => Self::InvalidTimestamp,
            ParseError::InvalidDeviceName => Self::InvalidDeviceName,
            ParseError::InvalidCanFrame => Self::InvalidCanFrame,
            ParseError::ConstructionError(e) => Self::ConstructionError(*e),
        }
    }
}

/// Hand-written because `serde(into = ...)` requires `Clone`, which
/// [`ParseError`] cannot have: `io::Error` is not `Clone`.
#[cfg(feature = "serde")]
impl Serialize for ParseError {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ParseErrorRepr::from(self).serialize(ser)
    }
}

#[cfg(feature = "serde")]
impl From<ParseErrorRepr> for ParseError {
    fn from(repr: ParseErrorRepr) -> Self {
        use crate::errors::io_kind_from_name;
        match repr {
            ParseErrorRepr::Io { kind, message } => {
                Self::Io(io::Error::new(io_kind_from_name(&kind), message))
            }
            ParseErrorRepr::UnexpectedEndOfLine => Self::UnexpectedEndOfLine,
            ParseErrorRepr::InvalidTimestamp => Self::InvalidTimestamp,
            ParseErrorRepr::InvalidDeviceName => Self::InvalidDeviceName,
            ParseErrorRepr::InvalidCanFrame => Self::InvalidCanFrame,
            ParseErrorRepr::ConstructionError(e) => Self::ConstructionError(e),
        }
    }
}

/// Recorded CAN frame.
/// This corresponds to the information in a line from the candump log.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CanDumpRecord {
    /// The timestamp
    pub t_us: u64,
    /// The name of the device
    pub device: String,
    /// The parsed frame
    pub frame: CanAnyFrame,
}

impl fmt::Display for CanDumpRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Split the microseconds with integer arithmetic rather than scaling
        // into an f64. candump's field is exactly six fractional digits and
        // the parser insists on that, so the seconds and the remainder are
        // written separately. Going through a float was exact only up to
        // about 4.29e15 µs (~year 2106); above that the last digit could
        // come back wrong, which a type that promises a byte-exact
        // round-trip cannot afford.
        write!(
            f,
            "({}.{:06}) {} ",
            self.t_us / 1_000_000,
            self.t_us % 1_000_000,
            self.device
        )?;

        // The frame body is rendered by the frame types themselves: their
        // `UpperHex` *is* the candump spelling — three hex digits of ID for
        // SFF and eight for EFF, `#R` for a remote frame, `##` and a flags
        // nibble for FD, `CAN_ERR_FLAG` included on an error frame — so a
        // record is just a timestamp, an interface, and `{:X}` of the frame.
        //
        // This used to be a second copy of that formatting, and the two drifted:
        // the copy here dropped `CAN_ERR_FLAG` from error frames, so its own
        // output read back as a standard data frame. One implementation, in
        // `frame.rs`, is what keeps `Display` and `{:X}` from disagreeing.
        fmt::UpperHex::fmt(&self.frame, f)
    }
}

/////////////////////////////////////////////////////////////////////////////
// Reader

#[derive(Debug)]
/// A CAN log reader.
pub struct Reader<R> {
    // The underlying reader
    rdr: R,
    // The line buffer
    buf: String,
}

impl<R: io::Read> Reader<R> {
    /// Creates an I/O buffered reader from a CAN log reader.
    pub fn from_reader(rdr: R) -> Reader<BufReader<R>> {
        Reader {
            rdr: BufReader::new(rdr),
            buf: String::with_capacity(256),
        }
    }
}

impl Reader<File> {
    /// Creates an I/O buffered reader from a file.
    pub fn from_file<P: AsRef<Path>>(path: P) -> io::Result<Reader<BufReader<File>>> {
        Ok(Reader::from_reader(File::open(path)?))
    }
}

impl<R: BufRead> Reader<R> {
    /// Advance state, returning next record.
    pub fn next_record(&mut self) -> Result<Option<CanDumpRecord>, ParseError> {
        // Cap each line at 64 KiB so a malformed/corrupt log can't OOM the
        // reader. A real candump line is on the order of ~120 bytes.
        const MAX_LINE: u64 = 64 * 1024;

        self.buf.clear();
        let mut handle = io::Read::take(&mut self.rdr, MAX_LINE);
        let nread = handle.read_line(&mut self.buf)?;

        // reached EOF
        if nread == 0 {
            return Ok(None);
        }

        // If we hit the cap without a trailing newline the line was over-long.
        if nread as u64 == MAX_LINE && !self.buf.ends_with('\n') {
            return Err(ParseError::InvalidCanFrame);
        }

        let line = self.buf[..nread].trim();
        let mut field_iter = line.split(' ');

        // parse timestamp field
        let ts = field_iter.next().ok_or(ParseError::UnexpectedEndOfLine)?;

        if ts.len() < 3 || !ts.starts_with('(') || !ts.ends_with(')') {
            return Err(ParseError::InvalidTimestamp);
        }

        let ts = &ts[1..ts.len() - 1];

        let t_us = match ts.split_once('.') {
            Some((num, mant)) => {
                // candump uses microsecond precision: exactly six digits after
                // the decimal point. Reject anything else rather than silently
                // misinterpreting the precision. Use checked arithmetic so a
                // pathological `num` doesn't overflow into a
                // wrong-but-valid-looking timestamp.
                if mant.len() != 6 {
                    return Err(ParseError::InvalidTimestamp);
                }
                let num = num
                    .parse::<u64>()
                    .map_err(|_| ParseError::InvalidTimestamp)?;
                let mant = mant
                    .parse::<u64>()
                    .map_err(|_| ParseError::InvalidTimestamp)?;
                num.checked_mul(1_000_000)
                    .and_then(|v| v.checked_add(mant))
                    .ok_or(ParseError::InvalidTimestamp)?
            }
            _ => return Err(ParseError::InvalidTimestamp),
        };

        // device name
        let device = field_iter
            .next()
            .ok_or(ParseError::UnexpectedEndOfLine)?
            .to_string();

        // parse packet
        let can_raw = field_iter.next().ok_or(ParseError::UnexpectedEndOfLine)?;

        let (can_id_str, mut can_data) = match can_raw.split_once('#') {
            Some((id, data)) => (id, data),
            _ => return Err(ParseError::InvalidCanFrame),
        };

        // The *width* of the ID field selects the frame format, not the
        // numeric value: candump writes exactly three hex digits for a
        // standard (SFF) identifier and exactly eight for an extended (EFF)
        // one, and can-utils' `parse_canframe()` decides the same way.
        //
        // Reading the value instead got this wrong in both directions, since
        // neither format's range is a subset of the other's spelling: an
        // eight-digit identifier of 0x7FF or less came back as a *standard*
        // frame, and a three-digit one above 0x7FF was promoted to an
        // *extended* frame — accepting an identifier that is not a legal
        // 11-bit ID at all. Neither round-tripped, because `Display` picks
        // the width back from the frame's own EFF flag.
        let is_extended = match can_id_str.len() {
            3 => false,
            8 => true,
            _ => return Err(ParseError::InvalidCanFrame),
        };

        let raw_id =
            canid_t::from_str_radix(can_id_str, 16).map_err(|_| ParseError::InvalidCanFrame)?;

        // Determine frame type (error, FD or classical) and skip separator(s)
        // Remember...
        //   Error:  "<canid|CAN_ERR_FLAG>#<8 class bytes>"
        //   CAN FD: "<canid>##<flags>[data]"
        //   Remote: "<canid>#R[len]"
        //   Data;   "<canid>#[data]"
        //
        // An eight-digit field is still ambiguous between an extended data
        // frame and an error frame; only CAN_ERR_FLAG tells those apart. The
        // error-class bits live above CAN_EFF_MASK, so the identifier
        // constructors below would reject them outright.

        let frame: CanAnyFrame = if raw_id & CAN_ERR_FLAG != 0 {
            // The payload is the error-class data bytes. Error frames are
            // always classical, so neither the FD nor the RTR form applies,
            // and `new_error` zero-pads a short payload out to the full
            // eight bytes the kernel always delivers.
            Vec::from_hex(can_data)
                .ok()
                .and_then(|data| CanErrorFrame::new_error(raw_id & CAN_ERR_MASK, &data).ok())
                .map(CanAnyFrame::Error)
        } else {
            // Each constructor range-checks its own format, so an
            // out-of-range value for the width that was written is rejected
            // rather than silently reinterpreted as the other format.
            let can_id: Id = if is_extended {
                CanId::extended(raw_id)
            } else {
                CanId::standard(raw_id as u16)
            }
            .ok_or(ParseError::InvalidCanFrame)?
            .into();

            if can_data.starts_with('#') {
                // `from_bits_retain`, not `from_bits_truncate`: the nibble
                // goes into `canfd_frame.flags` verbatim, the way can-utils
                // hands it over. Only three of the four bits have a name
                // today, and dropping the fourth would silently rewrite a
                // logged frame — `##F` would read back as `##7` and stop
                // round-tripping. An unnamed bit is preserved rather than
                // interpreted, as with `Location::Reserved`.
                let fd_flags = can_data
                    .get(1..2)
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
                    .map(FdFlags::from_bits_retain)
                    .ok_or(ParseError::InvalidCanFrame)?;
                Vec::from_hex(&can_data[2..])
                    .ok()
                    .and_then(|data| CanFdFrame::with_flags(can_id, &data, fd_flags))
                    .map(CanAnyFrame::Fd)
            } else if can_data.starts_with('R') {
                can_data = &can_data[1..];
                // Spec: the DLC after `R` is a single hex nibble (0..=F).
                // An empty tail is allowed and means DLC = 0.
                let rlen = if can_data.is_empty() {
                    0
                } else {
                    usize::from_str_radix(can_data, 16).map_err(|_| ParseError::InvalidCanFrame)?
                };
                CanRemoteFrame::new_remote(can_id, rlen)
                    .map(CanFrame::Remote)
                    .map(CanAnyFrame::from)
            } else {
                Vec::from_hex(can_data)
                    .ok()
                    .and_then(|data| CanDataFrame::new(can_id, &data))
                    .map(CanFrame::Data)
                    .map(CanAnyFrame::from)
            }
        }
        .ok_or(ParseError::InvalidCanFrame)?;

        Ok(Some(CanDumpRecord {
            t_us,
            device,
            frame,
        }))
    }
}

impl<R: BufRead> Iterator for Reader<R> {
    type Item = Result<CanDumpRecord, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        // lift Option:
        match self.next_record() {
            Ok(Some(rec)) => Some(Ok(rec)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

/////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod test {
    use super::*;
    use crate::{CanAnyFrame, Frame};
    use embedded_can::Frame as EmbeddedFrame;

    #[test]
    fn test_simple_example() {
        let input: &[u8] = b"(1469439874.299591) can1 080#\n\
                             (1469439874.299654) can1 701#7F";

        let mut reader = Reader::from_reader(input);

        let rec1 = reader.next_record().unwrap().unwrap();

        assert_eq!(rec1.t_us, 1469439874299591);
        assert_eq!(rec1.device, "can1");

        if let CanAnyFrame::Normal(frame) = rec1.frame {
            assert_eq!(frame.raw_id(), 0x080);
            assert!(!frame.is_remote_frame());
            assert!(!frame.is_error_frame());
            assert!(!frame.is_extended());
            assert_eq!(frame.data(), &[]);
        } else {
            panic!("Expected Normal frame, got FD");
        }

        let rec2 = reader.next_record().unwrap().unwrap();
        assert_eq!(rec2.t_us, 1469439874299654);
        assert_eq!(rec2.device, "can1");

        if let CanAnyFrame::Normal(frame) = rec2.frame {
            assert_eq!(frame.raw_id(), 0x701);
            assert!(!frame.is_remote_frame());
            assert!(!frame.is_error_frame());
            assert!(!frame.is_extended());
            assert_eq!(frame.data(), &[0x7F]);
        } else {
            panic!("Expected Normal frame, got FD");
        }

        assert!(reader.next_record().unwrap().is_none());
    }

    /// Extended identifiers, written the eight-digit way candump writes them.
    ///
    /// These lines used to carry six-digit identifiers, which no candump
    /// emits — the parser reached "extended" from the numeric value rather
    /// than the field width, so any width happened to work. It decides on
    /// width now, so the canonical spelling is what is exercised.
    #[test]
    fn test_extended_example() {
        let input: &[u8] = b"(1469439874.299591) can1 00080080#\n\
                             (1469439874.299654) can1 00053701#7F";

        let mut reader = Reader::from_reader(input);

        let rec1 = reader.next_record().unwrap().unwrap();

        assert_eq!(rec1.t_us, 1469439874299591);
        assert_eq!(rec1.device, "can1");

        if let CanAnyFrame::Normal(frame) = rec1.frame {
            assert_eq!(frame.raw_id(), 0x080080);
            assert!(!frame.is_remote_frame());
            assert!(!frame.is_error_frame());
            assert!(frame.is_extended());
            assert_eq!(frame.data(), &[]);
        } else {
            panic!("Expected Normal frame, got FD");
        }

        let rec2 = reader.next_record().unwrap().unwrap();
        assert_eq!(rec2.t_us, 1469439874299654);
        assert_eq!(rec2.device, "can1");

        if let CanAnyFrame::Normal(frame) = rec2.frame {
            assert_eq!(frame.raw_id(), 0x053701);
            assert!(!frame.is_remote_frame());
            assert!(!frame.is_error_frame());
            assert!(frame.is_extended());
            assert_eq!(frame.data(), &[0x7F]);
        } else {
            panic!("Expected Normal frame, got FD");
        }

        assert!(reader.next_record().unwrap().is_none());
    }

    #[test]
    fn test_remote() {
        let input: &[u8] = b"(1469439874.299591) can0 00080080#R\n\
                             (1469439874.299654) can0 00053701#R4";

        let mut reader = Reader::from_reader(input);

        let rec1 = reader.next_record().unwrap().unwrap();

        assert_eq!(rec1.t_us, 1469439874299591);
        assert_eq!(rec1.device, "can0");

        if let CanAnyFrame::Remote(frame) = rec1.frame {
            assert_eq!(frame.raw_id(), 0x080080);
            assert!(!frame.is_data_frame());
            assert!(frame.is_remote_frame());
            assert!(!frame.is_error_frame());
            assert!(frame.is_extended());
            assert_eq!(frame.len(), 0);
            assert_eq!(frame.data(), &[]);
        } else {
            panic!("Expected Remote frame");
        }

        let rec2 = reader.next_record().unwrap().unwrap();
        assert_eq!(rec2.t_us, 1469439874299654);
        assert_eq!(rec2.device, "can0");

        if let CanAnyFrame::Remote(frame) = rec2.frame {
            assert_eq!(frame.raw_id(), 0x053701);
            assert!(!frame.is_data_frame());
            assert!(frame.is_remote_frame());
            assert!(!frame.is_error_frame());
            assert!(frame.is_extended());
            assert_eq!(frame.len(), 4);
        } else {
            panic!("Expected Remote frame");
        }

        assert!(reader.next_record().unwrap().is_none());
    }

    /// The usable remote DLC range is `0..=8`, and every value in it
    /// round-trips byte for byte.
    ///
    /// The wire field is four bits wide, but SocketCAN caps a classical
    /// frame's length at `CAN_MAX_DLEN` in both directions — the kernel
    /// refuses a longer one with `EINVAL` on send — so a higher nibble
    /// describes a frame that could never be transmitted. See
    /// `doc/CanDumpLogFormat.md` for why this parser rejects it where
    /// can-utils discards it.
    #[test]
    fn test_remote_dlc_range() {
        for dlc in 0..=8usize {
            let line = if dlc == 0 {
                "(1469439874.299591) can0 123#R".to_string()
            } else {
                format!("(1469439874.299591) can0 123#R{:X}", dlc)
            };

            let mut reader = Reader::from_reader(line.as_bytes());
            let rec = reader.next_record().unwrap().unwrap();

            match rec.frame {
                CanAnyFrame::Remote(frame) => assert_eq!(frame.dlc(), dlc, "{line}"),
                other => panic!("expected a remote frame, got {other:?}"),
            }
            assert_eq!(rec.to_string(), line, "round-trip changed the line");
        }

        // Above the kernel's limit: rejected, not silently taken as DLC 0.
        for line in [
            "(1469439874.299591) can0 123#R9",
            "(1469439874.299591) can0 123#RF",
        ] {
            let mut reader = Reader::from_reader(line.as_bytes());
            assert!(
                matches!(reader.next_record(), Err(ParseError::InvalidCanFrame)),
                "{line} should be rejected"
            );
        }

        // The same boundary at its source, so the reason stays visible.
        assert!(CanRemoteFrame::remote_from_raw_id(0x123, 8).is_some());
        assert!(CanRemoteFrame::remote_from_raw_id(0x123, 9).is_none());
    }

    // Issue #74
    #[test]
    fn test_extended_id_fd() {
        let input: &[u8] = b"(1234.567890) can0 12345678##500112233445566778899AABB";

        let mut reader = Reader::from_reader(input);
        let rec = reader.next_record().unwrap().unwrap();
        let frame = CanFdFrame::try_from(rec.frame).unwrap();

        assert!(frame.is_extended());
        assert_eq!(0x12345678, frame.raw_id());
        assert_eq!(5, frame.flags().bits());
        assert_eq!(frame.dlc(), 0x09);
        assert_eq!(frame.len(), 12);
        assert_eq!(frame.data().len(), 12);
        assert_eq!(
            frame.data(),
            &[
                0x0, 0x011, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB
            ]
        );

        // The FD form round-trips, flags nibble and all.
        assert_eq!(
            rec.to_string(),
            "(1234.567890) can0 12345678##500112233445566778899AABB"
        );
    }

    /// Error-frame lines, captured verbatim from
    /// `candump -L vcan0,0:0,#FFFFFFFF` with frames injected by `cansend`.
    ///
    /// Note the ID field carries `CAN_ERR_FLAG` (`0x2000_0000`), which is
    /// what distinguishes these from the eight-digit *extended* IDs in
    /// `test_extended_example` — the field width alone is ambiguous.
    #[test]
    fn test_error_frames() {
        let input: &[u8] = b"(1785099856.242430) vcan0 20000004#000C000000000000\n\
                             (1785099856.243595) vcan0 200000A8#00009E0800000000";

        let mut reader = Reader::from_reader(input);

        // CAN_ERR_CRTL with both warning bits set in data[1] — what the
        // kernel's can_change_state() emits on a symmetric transition.
        let rec1 = reader.next_record().unwrap().unwrap();
        assert_eq!(rec1.t_us, 1785099856242430);
        assert_eq!(rec1.device, "vcan0");

        if let CanAnyFrame::Error(frame) = rec1.frame {
            assert!(frame.is_error_frame());
            assert!(!frame.is_data_frame());
            assert!(!frame.is_remote_frame());
            assert_eq!(frame.error_bits(), 0x004);
            assert_eq!(frame.data(), &[0, 0x0C, 0, 0, 0, 0, 0, 0]);
            // ... and it decodes through the v4 errors path. The symmetric
            // warning transition folds into a single Controller cause.
            let err = frame.into_error();
            assert_eq!(err.len(), 1);
        } else {
            panic!("Expected Error frame, got {:?}", rec1.frame);
        }

        // CAN_ERR_PROT | CAN_ERR_BUSERROR | CAN_ERR_ACK, five violation bits.
        let rec2 = reader.next_record().unwrap().unwrap();
        assert_eq!(rec2.t_us, 1785099856243595);

        if let CanAnyFrame::Error(frame) = rec2.frame {
            assert_eq!(frame.error_bits(), 0x0A8);
            assert_eq!(frame.data(), &[0, 0, 0x9E, 0x08, 0, 0, 0, 0]);
            // Protocol violations fold into one cause: Protocol + NoAck + BusError.
            assert_eq!(frame.into_error().len(), 3);
        } else {
            panic!("Expected Error frame, got {:?}", rec2.frame);
        }

        assert!(reader.next_record().unwrap().is_none());
    }

    /// An error-frame line must survive parse → `Display` → parse unchanged.
    ///
    /// Before the grammar-parity fix neither direction worked: the parser
    /// rejected the line outright (the class bits sit above `CAN_EFF_MASK`,
    /// so `id_from_raw` refused them), and `Display` stripped
    /// `CAN_ERR_FLAG`, so its own output re-parsed as a *standard data
    /// frame* rather than an error frame.
    #[test]
    fn test_error_frame_round_trip() {
        const LINES: &[&str] = &[
            "(1785099856.242430) vcan0 20000004#000C000000000000",
            "(1785099856.243595) vcan0 200000A8#00009E0800000000",
            // Transceiver status, both nibbles set (etas_es58x).
            "(1785099856.244721) vcan0 20000010#0000000044000000",
            // Bus off, no detail bytes.
            "(1785099856.245800) vcan0 20000040#0000000000000000",
            // Every class bit this crate knows, including CAN_ERR_CNT.
            "(1785099856.246900) vcan0 200003FF#070C9E08440A7060",
        ];

        for line in LINES {
            let mut reader = Reader::from_reader(line.as_bytes());
            let rec = reader.next_record().unwrap().unwrap();
            assert!(matches!(rec.frame, CanAnyFrame::Error(_)), "{line}");
            assert_eq!(rec.to_string(), *line, "round-trip changed the line");
        }
    }

    /// An eight-digit ID *without* `CAN_ERR_FLAG` is still an extended data
    /// frame. Guards against the error branch swallowing extended IDs.
    #[test]
    fn test_extended_id_is_not_mistaken_for_error() {
        let input: &[u8] = b"(1785099856.241425) vcan0 1FFFFFFF#0102";

        let mut reader = Reader::from_reader(input);
        let rec = reader.next_record().unwrap().unwrap();

        if let CanAnyFrame::Normal(frame) = rec.frame {
            assert!(frame.is_extended());
            assert!(!frame.is_error_frame());
            assert_eq!(frame.raw_id(), 0x1FFFFFFF);
            assert_eq!(frame.data(), &[0x01, 0x02]);
        } else {
            panic!("Expected Normal frame, got {:?}", rec.frame);
        }
        assert_eq!(rec.to_string(), "(1785099856.241425) vcan0 1FFFFFFF#0102");
    }

    /// Every microsecond timestamp survives `Display` exactly, including
    /// values a float could not carry.
    ///
    /// The rendering used to scale `t_us` into an `f64`, which was exact only
    /// below about 4.29e15 µs (~year 2106): `8014677457392536` came back as
    /// `8014677457392535`. Well past any real capture, but the parser demands
    /// exactly six fractional digits and this type promises a byte-exact
    /// round-trip, so the arithmetic is done in integers.
    #[test]
    fn test_timestamp_round_trip_is_exact() {
        const CASES: &[u64] = &[
            0,
            1,
            999_999,
            1_000_000,
            1_785_099_856_242_430, // a present-day capture
            4_294_967_295_004_142, // where the float path first broke
            8_014_677_457_392_536, // and well beyond it
            u64::MAX / 2,
        ];

        for &t_us in CASES {
            let line = format!("({}.{:06}) can0 123#01", t_us / 1_000_000, t_us % 1_000_000);
            let mut reader = Reader::from_reader(line.as_bytes());
            let rec = reader
                .next_record()
                .unwrap_or_else(|e| panic!("{line}: {e}"))
                .expect("a record");

            assert_eq!(rec.t_us, t_us, "{line}");
            assert_eq!(rec.to_string(), line, "round-trip changed the line");
        }
    }

    /// A record's frame body is exactly `{:X}` of the frame.
    ///
    /// `Display` delegates to the frame types' `UpperHex` rather than keeping
    /// a second copy of the candump formatting. The two used to be separate
    /// implementations and drifted: the copy in this module dropped
    /// `CAN_ERR_FLAG` from error frames. This pins the relationship so a
    /// re-split would have to break a test to happen quietly.
    #[test]
    fn test_record_body_is_frame_upper_hex() {
        const BODIES: &[&str] = &[
            "123#",
            "123#01",
            "7FF#1122334455667788",
            "00000123#01",
            "1FFFFFFF#0102",
            "123#R",
            "123#R4",
            "00000123#R8",
            // The FDF bit (0x4) is set in each of these: `CanFdFrame` forces
            // it on at construction, so a nibble without it does not survive
            // the trip. See `test_fd_flag_nibble_is_preserved`.
            "123##4",
            "123##500112233",
            "00000123##F00",
            "20000004#000C000000000000",
            "200003FF#070C9E08440A7060",
        ];

        for body in BODIES {
            let line = format!("(1469439874.299591) can0 {body}");
            let mut reader = Reader::from_reader(line.as_bytes());
            let rec = reader
                .next_record()
                .unwrap_or_else(|e| panic!("{line}: {e}"))
                .expect("a record");

            assert_eq!(
                format!("{:X}", rec.frame),
                *body,
                "{line}: frame's own rendering"
            );
            assert_eq!(rec.to_string(), line, "{line}: record rendering");
        }
    }

    /// The FD flags nibble reaches the frame intact, including the bit with
    /// no name.
    ///
    /// Only `BRS` (0x1), `ESI` (0x2) and `FDF` (0x4) are defined; the fourth
    /// bit of the nibble is unassigned. Decoding it with
    /// `FdFlags::from_bits_truncate` silently dropped it, so a logged `##F`
    /// came back as `##7` — a different frame. No kernel sets that bit today,
    /// which is exactly why it would have gone unnoticed.
    ///
    /// The one bit that does *not* survive untouched is `FDF`, which
    /// [`CanFdFrame`] forces on at construction, as its type documentation
    /// states. A nibble that already carries it therefore round-trips
    /// exactly; one that does not comes back with it added.
    ///
    /// The raw `canfd_frame.flags` byte is what gets asserted here, because
    /// the [`CanFdFrame::flags()`] getter runs `from_bits_truncate` and so
    /// under-reports the unnamed bit that the frame is in fact carrying.
    #[test]
    fn test_fd_flag_nibble_is_preserved() {
        const FDF: u8 = 0x4;

        for nibble in 0..=0xFu8 {
            let line = format!("(1469439874.299591) can0 123##{nibble:X}00");

            let mut reader = Reader::from_reader(line.as_bytes());
            let rec = reader.next_record().unwrap().expect("a record");

            match &rec.frame {
                CanAnyFrame::Fd(frame) => {
                    assert_eq!(
                        frame.as_ref().flags,
                        nibble | FDF,
                        "{line}: every bit but the forced FDF must survive"
                    );
                }
                other => panic!("{line}: expected an FD frame, got {other:?}"),
            }

            let expected = format!("(1469439874.299591) can0 123##{:X}00", nibble | FDF);
            assert_eq!(rec.to_string(), expected, "{line}");
        }
    }

    /// The identifier's *width* picks the frame format, not its value.
    ///
    /// The two formats' spellings are not nested, so deciding on the value
    /// was wrong in both directions: an eight-digit identifier of `0x7FF` or
    /// less came back standard (`00000123#01` re-emitted as `123#01`), and a
    /// three-digit one above `0x7FF` came back extended (`800#AA` re-emitted
    /// as `00000800#AA`) — the latter accepting a value that is not a legal
    /// 11-bit identifier at all.
    #[test]
    fn test_id_width_selects_the_format() {
        // (line body, extended?) — every one of these round-trips.
        let cases: &[(&str, bool)] = &[
            ("123#01", false),
            ("7FF#01", false),
            ("000#01", false),
            ("00000123#01", true), // low value, still extended
            ("00000000#01", true),
            ("000007FF#01", true), // exactly at the SFF boundary
            ("1FFFFFFF#0102", true),
            ("00000123#R4", true), // and for remote frames
            ("123#R4", false),
            ("00000123##500112233", true), // and FD
            ("123##500112233", false),
        ];

        for (body, extended) in cases {
            let line = format!("(1469439874.299591) can0 {body}");
            let mut reader = Reader::from_reader(line.as_bytes());
            let rec = reader
                .next_record()
                .unwrap_or_else(|e| panic!("{line}: {e}"))
                .expect("a record");

            let got = match &rec.frame {
                CanAnyFrame::Normal(f) => f.is_extended(),
                CanAnyFrame::Remote(f) => f.is_extended(),
                CanAnyFrame::Fd(f) => f.is_extended(),
                other => panic!("{line}: unexpected {other:?}"),
            };
            assert_eq!(got, *extended, "{line}: wrong format");
            assert_eq!(rec.to_string(), line, "round-trip changed the line");
        }

        // A three-digit field cannot hold an identifier above CAN_SFF_MASK,
        // and is rejected rather than quietly promoted to extended.
        // Likewise an eight-digit one above CAN_EFF_MASK, once the error
        // flag is excluded.
        for body in ["800#AA", "FFF#AA", "1FFFFFFF0#01"] {
            let line = format!("(1469439874.299591) can0 {body}");
            let mut reader = Reader::from_reader(line.as_bytes());
            assert!(
                matches!(reader.next_record(), Err(ParseError::InvalidCanFrame)),
                "{line} should be rejected"
            );
        }

        // Only 3 and 8 are identifier widths; candump emits nothing else.
        for width in [1usize, 2, 4, 5, 6, 7, 9] {
            let line = format!("(1469439874.299591) can0 {}#01", "0".repeat(width));
            let mut reader = Reader::from_reader(line.as_bytes());
            assert!(
                matches!(reader.next_record(), Err(ParseError::InvalidCanFrame)),
                "{width}-digit identifier should be rejected"
            );
        }
    }

    /// Pins the documented limitation that the `_len8_dlc` suffix is not
    /// supported. If this starts failing because the form was implemented,
    /// update the module docs and `doc/CanDumpLogFormat.md` to match.
    #[test]
    fn test_len8_dlc_suffix_is_rejected() {
        let input: &[u8] = b"(1469439874.299591) can1 123#1122334455667788_E";

        let mut reader = Reader::from_reader(input);
        assert!(matches!(
            reader.next_record(),
            Err(ParseError::InvalidCanFrame)
        ));
    }

    #[test]
    fn test_fd() {
        let input: &[u8] = b"(1469439874.299591) can1 080##0\n\
                             (1469439874.299654) can1 701##17F";

        let mut reader = Reader::from_reader(input);

        let rec1 = reader.next_record().unwrap().unwrap();

        assert_eq!(rec1.t_us, 1469439874299591);
        assert_eq!(rec1.device, "can1");
        if let CanAnyFrame::Fd(frame) = rec1.frame {
            assert_eq!(frame.raw_id(), 0x080);
            assert!(!frame.is_remote_frame());
            assert!(!frame.is_error_frame());
            assert!(!frame.is_extended());
            assert!(!frame.is_brs());
            assert!(!frame.is_esi());
            assert_eq!(0x04, frame.flags().bits());
            assert_eq!(frame.dlc(), 0);
            assert_eq!(frame.len(), 0);
            assert_eq!(frame.data().len(), 0);
            assert_eq!(frame.data(), &[]);
        } else {
            panic!("Expected FD frame, got Normal");
        }

        let rec2 = reader.next_record().unwrap().unwrap();
        assert_eq!(rec2.t_us, 1469439874299654);
        assert_eq!(rec2.device, "can1");
        if let CanAnyFrame::Fd(frame) = rec2.frame {
            assert_eq!(frame.raw_id(), 0x701);
            assert!(!frame.is_remote_frame());
            assert!(!frame.is_error_frame());
            assert!(!frame.is_extended());
            assert!(frame.is_brs());
            assert!(!frame.is_esi());
            assert_eq!(0x05, frame.flags().bits());
            assert_eq!(frame.dlc(), 1);
            assert_eq!(frame.len(), 1);
            assert_eq!(frame.data().len(), 1);
            assert_eq!(frame.data(), &[0x7F]);
        } else {
            panic!("Expected FD frame, got Normal");
        }

        assert!(reader.next_record().unwrap().is_none());
    }
}
