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
//! rtr       = "R" [ HEXDIG ]   ; remote frame; optional DLC nibble 0..F
//! data      = *(2HEXDIG)       ; 0..8 bytes classical, 0..64 bytes FD
//! flags     = HEXDIG           ; canfd_frame.flags: BRS=0x1 ESI=0x2 FDF=0x4
//! dlc8      = HEXDIG           ; "len8 DLC" escape; see the caveat below
//! ```
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
    CanAnyFrame, CanDataFrame, CanErrorFrame, CanFdFrame, CanFrame, CanRemoteFrame,
    ConstructionError,
    frame::Frame,
    id::{CAN_ERR_FLAG, CAN_ERR_MASK, FdFlags, id_from_raw},
};
use embedded_can::Frame as EmbeddedFrame;
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
        // Width of the ID field matches candump's log format: 3 hex chars
        // for SFF, 8 for EFF. Error frames use the full 29-bit error mask
        // width so future kernel additions outside CAN_SFF_MASK still render.
        write!(f, "({:.6}) {} ", 1.0e-6 * self.t_us as f64, self.device)?;
        use CanAnyFrame::*;
        match self.frame {
            Remote(frame) if frame.len() == 0 => {
                Self::fmt_id(f, &frame)?;
                f.write_str("#R")
            }
            Remote(frame) => {
                Self::fmt_id(f, &frame)?;
                write!(f, "#R{:X}", frame.dlc())
            }
            Error(frame) => {
                // candump logs error frames with CAN_ERR_FLAG *included*, so
                // the eight-digit field reads e.g. `20000004#…` and feeds
                // straight back through the parser. Emitting the bare class
                // bits instead would re-parse as a standard data frame.
                write!(f, "{:08X}#", frame.error_bits() | CAN_ERR_FLAG)?;
                Self::fmt_data(f, frame.data())
            }
            Normal(frame) => {
                Self::fmt_id(f, &frame)?;
                f.write_str("#")?;
                Self::fmt_data(f, frame.data())
            }
            Fd(frame) => {
                Self::fmt_id(f, &frame)?;
                // Single-hex-nibble flags between `##` and the payload, matching
                // candump's `.log` format so this output round-trips through
                // the parser.
                write!(f, "##{:X}", frame.flags().bits() & 0x0F)?;
                Self::fmt_data(f, frame.data())
            }
        }
    }
}

impl CanDumpRecord {
    /// Writes the CAN ID with candump-style zero padding: 3 hex chars for
    /// standard IDs, 8 for extended.
    fn fmt_id<F: Frame>(f: &mut fmt::Formatter<'_>, frame: &F) -> fmt::Result {
        if frame.is_extended() {
            write!(f, "{:08X}", frame.raw_id())
        } else {
            write!(f, "{:03X}", frame.raw_id())
        }
    }

    /// Writes a payload as candump's unseparated, uppercase hex.
    fn fmt_data(f: &mut fmt::Formatter<'_>, data: &[u8]) -> fmt::Result {
        data.iter().try_for_each(|b| write!(f, "{:02X}", b))
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
    /// Returns an iterator over all records
    #[deprecated(since = "3.5.0", note = "Use `iter()`")]
    pub fn records(&mut self) -> CanDumpRecords<'_, R> {
        CanDumpRecords { src: self }
    }

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

        // Parse the raw ID word.
        //
        // An eight-digit ID field is ambiguous on its face: it is an error
        // frame if CAN_ERR_FLAG is set and an extended data frame otherwise.
        // Resolve that before anything else, mirroring how can-utils'
        // `parse_canframe()` decides (it only adds CAN_EFF_FLAG when
        // CAN_ERR_FLAG is absent). The error-class bits live above
        // CAN_EFF_MASK, so `id_from_raw` would reject them outright.
        let raw_id =
            canid_t::from_str_radix(can_id_str, 16).map_err(|_| ParseError::InvalidCanFrame)?;

        // Determine frame type (error, FD or classical) and skip separator(s)
        // Remember...
        //   Error:  "<canid|CAN_ERR_FLAG>#<8 class bytes>"
        //   CAN FD: "<canid>##<flags>[data]"
        //   Remote: "<canid>#R[len]"
        //   Data;   "<canid>#[data]"

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
            let can_id = id_from_raw(raw_id).ok_or(ParseError::InvalidCanFrame)?;

            if can_data.starts_with('#') {
                let fd_flags = can_data
                    .get(1..2)
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
                    .map(FdFlags::from_bits_truncate)
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

/// Original Record iterator
#[derive(Debug)]
pub struct CanDumpRecords<'a, R: 'a> {
    src: &'a mut Reader<R>,
}

impl<R: io::Read> Iterator for CanDumpRecords<'_, BufReader<R>> {
    type Item = Result<(u64, CanAnyFrame), ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        // lift Option:
        match self.src.next_record() {
            Ok(Some(CanDumpRecord { t_us, frame, .. })) => Some(Ok((t_us, frame))),
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

    #[test]
    fn test_extended_example() {
        let input: &[u8] = b"(1469439874.299591) can1 080080#\n\
                             (1469439874.299654) can1 053701#7F";

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
        let input: &[u8] = b"(1469439874.299591) can0 080080#R\n\
                             (1469439874.299654) can0 053701#R4";

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
