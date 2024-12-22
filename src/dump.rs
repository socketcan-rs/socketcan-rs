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
    CanDataFrame, CanFdFrame,
};
use embedded_can::StandardId;
use hex::FromHex;
use libc::canid_t;
use std::{fs, io, path};

// cannot be generic, because from_str_radix is not part of any Trait
fn parse_raw(bytes: &[u8], radix: u32) -> Option<u64> {
    ::std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| u64::from_str_radix(s, radix).ok())
}

#[derive(Debug)]
/// A CAN log reader.
pub struct Reader<R> {
    rdr: R,
    line_buf: Vec<u8>,
}

impl<R: io::Read> Reader<R> {
    /// Creates an I/O buffered reader from a CAN log reader.
    pub fn from_reader(rdr: R) -> Reader<io::BufReader<R>> {
        Reader {
            rdr: io::BufReader::new(rdr),
            line_buf: Vec::new(),
        }
    }
}

impl Reader<fs::File> {
    /// Creates an I/O buffered reader from a file.
    pub fn from_file<P>(path: P) -> io::Result<Reader<io::BufReader<fs::File>>>
    where
        P: AsRef<path::Path>,
    {
        Ok(Reader::from_reader(fs::File::open(path)?))
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
    pub frame: super::CanAnyFrame,
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
    ConstructionError(super::ConstructionError),
}

impl From<io::Error> for ParseError {
    fn from(e: io::Error) -> ParseError {
        ParseError::Io(e)
    }
}

impl From<super::ConstructionError> for ParseError {
    fn from(e: super::ConstructionError) -> ParseError {
        ParseError::ConstructionError(e)
    }
}

impl<R: io::BufRead> Reader<R> {
    /// Returns an iterator over all records
    pub fn records(&mut self) -> CanDumpRecords<R> {
        CanDumpRecords { src: self }
    }

    /// Advance state, returning next record.
    pub fn next_record(&mut self) -> Result<Option<CanDumpRecord>, ParseError> {
        self.line_buf.clear();
        let bytes_read = self.rdr.read_until(b'\n', &mut self.line_buf)?;

        // reached EOF
        if bytes_read == 0 {
            return Ok(None);
        }

        let mut field_iter = self.line_buf.split(|&c| c == b' ');

        // parse time field
        let f = field_iter.next().ok_or(ParseError::UnexpectedEndOfLine)?;

        if f.len() < 3 || f[0] != b'(' || f[f.len() - 1] != b')' {
            return Err(ParseError::InvalidTimestamp);
        }

        let inner = &f[1..f.len() - 1];

        // split at dot, read both parts
        let dot = inner
            .iter()
            .position(|&c| c == b'.')
            .ok_or(ParseError::InvalidTimestamp)?;

        let (num, mant) = inner.split_at(dot);

        // parse number and multiply
        let n_num: u64 = parse_raw(num, 10).ok_or(ParseError::InvalidTimestamp)?;
        let n_mant: u64 = parse_raw(&mant[1..], 10).ok_or(ParseError::InvalidTimestamp)?;
        let t_us = n_num.saturating_mul(1_000_000).saturating_add(n_mant);

        let f = field_iter.next().ok_or(ParseError::UnexpectedEndOfLine)?;

        // device name
        let device = ::std::str::from_utf8(f).map_err(|_| ParseError::InvalidDeviceName)?;

        // parse packet
        let can_raw = field_iter.next().ok_or(ParseError::UnexpectedEndOfLine)?;

        let sep_idx = can_raw
            .iter()
            .position(|&c| c == b'#')
            .ok_or(ParseError::InvalidCanFrame)?;
        let (can_id, mut can_data) = can_raw.split_at(sep_idx);

        // determine frame type (FD or classical) and skip separator(s)
        let mut fd_flags = FdFlags::empty();
        let is_fd_frame = if let Some(&b'#') = can_data.get(1) {
            fd_flags = FdFlags::from_bits_truncate(can_data[2]);
            can_data = &can_data[3..];
            true
        } else {
            can_data = &can_data[1..];
            false
        };

        // cut of linefeed
        if let Some(&b'\n') = can_data.last() {
            can_data = &can_data[..can_data.len() - 1];
        };
        // cut off \r
        if let Some(&b'\r') = can_data.last() {
            can_data = &can_data[..can_data.len() - 1];
        };

        let mut flags = IdFlags::empty();
        flags.set(IdFlags::RTR, b"R" == can_data);
        // TODO: How are error frames saved?

        let data = if flags.contains(IdFlags::RTR) {
            Vec::new()
        } else {
            Vec::from_hex(can_data).map_err(|_| ParseError::InvalidCanFrame)?
        };
        let frame: super::CanAnyFrame = if is_fd_frame {
            CanFdFrame::init(
                parse_raw(can_id, 16).ok_or(ParseError::InvalidCanFrame)? as canid_t | flags.bits(),
                &data,
                fd_flags,
            )
            .map(super::CanAnyFrame::Fd)
        } else {
            // TODO: Check for other frame types?
            // is extended?
            let can_id = parse_raw(can_id, 16).ok_or(ParseError::InvalidCanFrame)?;
            if can_id >= StandardId::MAX.as_raw() as u64 {
                flags.set(IdFlags::EFF, true);
            }
            CanDataFrame::init(can_id as canid_t | flags.bits(), &data)
                .map(super::CanFrame::Data)
                .map(|f| f.into())
        }?;

        Ok(Some(CanDumpRecord {
            t_us,
            device,
            frame,
        }))
    }
}

impl<R: io::Read> Iterator for CanDumpRecords<'_, io::BufReader<R>> {
    type Item = Result<(u64, super::CanAnyFrame), ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        // lift Option:
        match self.src.next_record() {
            Ok(Some(CanDumpRecord { t_us, frame, .. })) => Some(Ok((t_us, frame))),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

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

        {
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
        }

        {
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
        }

        assert!(reader.next_record().unwrap().is_none());
    }

    #[test]
    fn test_extended_example() {
        let input: &[u8] = b"(1469439874.299591) can1 080080#\n\
                             (1469439874.299654) can1 053701#7F";

        let mut reader = Reader::from_reader(input);

        {
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
        }

        {
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
        }

        assert!(reader.next_record().unwrap().is_none());
    }

    #[test]
    fn test_fd() {
        let input: &[u8] = b"(1469439874.299591) can1 080##0\n\
                             (1469439874.299654) can1 701##17F";

        let mut reader = Reader::from_reader(input);

        {
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
        }

        {
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
        }

        assert!(reader.next_record().unwrap().is_none());
    }
}
