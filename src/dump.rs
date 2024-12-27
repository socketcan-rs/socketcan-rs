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
//! Parses the text format emitted by the `candump` utility, which is part of
//! [can-utils](https://github.com/linux-can/can-utils).
//!
//! Example:
//!
//! ```text
//! (1469439874.299654) can1 701#7F
//! ```
//!
//! Can be parsed by a `Reader` object. The API is inspired by the
//! [csv](https://crates.io/crates/csv) crate.

use crate::{
    id::{FdFlags, IdFlags},
    CanAnyFrame, CanDataFrame, CanFdFrame, CanFrame, ConstructionError,
};
use embedded_can::StandardId;
use hex::FromHex;
use lazy_static::lazy_static;
use libc::canid_t;
use regex::Regex;
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
    str,
};

#[derive(Debug)]
/// A CAN log reader.
pub struct Reader<R> {
    rdr: R,
    line_buf: Vec<u8>,
}

impl<R: io::Read> Reader<R> {
    /// Creates an I/O buffered reader from a CAN log reader.
    pub fn from_reader(rdr: R) -> Reader<BufReader<R>> {
        Reader {
            rdr: BufReader::new(rdr),
            line_buf: Vec::new(),
        }
    }
}

impl Reader<File> {
    /// Creates an I/O buffered reader from a file.
    pub fn from_file<P: AsRef<Path>>(path: P) -> io::Result<Reader<BufReader<File>>> {
        Ok(Reader::from_reader(File::open(path)?))
    }
}

/// Record iterator
#[derive(Debug)]
pub struct CanDumpRecords<'a, R: 'a> {
    src: &'a mut Reader<R>,
}

/// Recorded CAN frame.
#[derive(Debug)]
pub struct CanDumpRecord<'a> {
    /// The timestamp
    pub t_us: u64,
    /// The name of the device
    pub device: &'a str,
    /// The parsed frame
    pub frame: CanAnyFrame,
}

#[derive(Debug)]
/// candump line parse error
pub enum ParseError {
    /// I/O Error
    Io(io::Error),
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

impl From<io::Error> for ParseError {
    fn from(e: io::Error) -> ParseError {
        ParseError::Io(e)
    }
}

impl From<ConstructionError> for ParseError {
    fn from(e: ConstructionError) -> ParseError {
        ParseError::ConstructionError(e)
    }
}

lazy_static! {
    // Matches a candump line
    static ref RE_DUMP: Regex = Regex::new(
        r"\s*\((?P<t_num>[0-9]+)\.(?P<t_mant>[0-9]*)\)\s+(?P<iface>\w+)\s+(?P<can_id>[0-9A-Fa-f]+)(((?P<fd_sep>\#\#)(?P<fd_flags>[0-9A-Fa-f]))|(?P<sep>\#))(?P<data>[0-9A-Fa-f\s]*)"
    ).unwrap();
}

impl<R: BufRead> Reader<R> {
    /// Returns an iterator over all records
    pub fn records(&mut self) -> CanDumpRecords<R> {
        CanDumpRecords { src: self }
    }

    /// Advance state, returning next record.
    pub fn next_record(&mut self) -> Result<Option<CanDumpRecord>, ParseError> {
        self.line_buf.clear();
        let bytes_read = self.rdr.read_until(b'\n', &mut self.line_buf)?;

        if bytes_read == 0 {
            return Ok(None);
        }

        let line = str::from_utf8(&self.line_buf[..bytes_read])
            .map_err(|_| ParseError::InvalidCanFrame)?;

        let caps = RE_DUMP
            .captures(line)
            .ok_or(ParseError::UnexpectedEndOfLine)?;

        let t_num: u64 = caps
            .name("t_num")
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .ok_or(ParseError::InvalidTimestamp)?;

        let t_mant: u64 = caps
            .name("t_mant")
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .ok_or(ParseError::InvalidTimestamp)?;

        let t_us = t_num.saturating_mul(1_000_000).saturating_add(t_mant);

        let device = caps
            .name("iface")
            .map(|m| m.as_str())
            //.map(String::from)
            .ok_or(ParseError::InvalidDeviceName)?;

        let is_fd_frame = caps.name("fd_sep").is_some();

        let mut can_id: canid_t = caps
            .name("can_id")
            .and_then(|m| canid_t::from_str_radix(m.as_str(), 16).ok())
            .ok_or(ParseError::InvalidCanFrame)?;

        let can_data = caps
            .name("data")
            .map(|m| m.as_str().trim())
            .ok_or(ParseError::InvalidCanFrame)?;

        let mut id_flags = IdFlags::empty();
        id_flags.set(IdFlags::RTR, "R" == can_data);
        id_flags.set(IdFlags::EFF, can_id >= StandardId::MAX.as_raw() as canid_t);
        // TODO: How are error frames saved?
        can_id |= id_flags.bits();

        let data = match id_flags.contains(IdFlags::RTR) {
            true => vec![],
            false => Vec::from_hex(can_data).map_err(|_| ParseError::InvalidCanFrame)?,
        };

        let frame: CanAnyFrame = if is_fd_frame {
            let fd_flags: FdFlags = caps
                .name("fd_flags")
                .and_then(|m| u8::from_str_radix(m.as_str(), 16).ok())
                .map(FdFlags::from_bits_truncate)
                .ok_or(ParseError::InvalidCanFrame)?;

            CanFdFrame::init(can_id, &data, fd_flags).map(CanAnyFrame::Fd)
        } else {
            CanDataFrame::init(can_id, &data)
                .map(CanFrame::Data)
                .map(CanAnyFrame::from)
        }?;

        Ok(Some(CanDumpRecord {
            t_us,
            device,
            frame,
        }))
    }
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
            assert_eq!(frame.is_remote_frame(), false);
            assert_eq!(frame.is_error_frame(), false);
            assert_eq!(frame.is_extended(), true);
            assert_eq!(frame.data(), &[]);
        } else {
            panic!("Expected Normal frame, got FD");
        }

        let rec2 = reader.next_record().unwrap().unwrap();
        assert_eq!(rec2.t_us, 1469439874299654);
        assert_eq!(rec2.device, "can1");

        if let CanAnyFrame::Normal(frame) = rec2.frame {
            assert_eq!(frame.raw_id(), 0x053701);
            assert_eq!(frame.is_remote_frame(), false);
            assert_eq!(frame.is_error_frame(), false);
            assert_eq!(frame.is_extended(), true);
            assert_eq!(frame.data(), &[0x7F]);
        } else {
            panic!("Expected Normal frame, got FD");
        }

        assert!(reader.next_record().unwrap().is_none());
    }

    // Issue #74
    #[test]
    fn test_extended_id_fd() {
        let input: &[u8] = b"(1234.567890) can0 12345678##500112233445566778899AABBCCDDEEFF";

        let mut reader = Reader::from_reader(input);
        let rec = reader.next_record().unwrap().unwrap();
        let frame = CanFdFrame::try_from(rec.frame).unwrap();

        assert!(frame.is_extended());
        assert_eq!(0x12345678, frame.raw_id());
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
            assert_eq!(frame.data(), &[0x7F]);
        } else {
            panic!("Expected FD frame, got Normal");
        }

        assert!(reader.next_record().unwrap().is_none());
    }
}
