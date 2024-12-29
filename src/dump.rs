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
//! Parses the text log format emitted by the `candump` utility, which is
//! part of [can-utils](https://github.com/linux-can/can-utils).
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
//! ```
//!
//! Can be parsed by a `Reader` object. The API is inspired by the
//! [csv](https://crates.io/crates/csv) crate.

use crate::{
    frame::Frame,
    id::{id_from_raw, FdFlags},
    CanAnyFrame, CanDataFrame, CanFdFrame, CanFrame, CanRemoteFrame, ConstructionError,
};
use embedded_can::Frame as EmbeddedFrame;
use hex::FromHex;
use itertools::Itertools;
use libc::canid_t;
use std::{
    fmt,
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
};
use thiserror::Error;

/// candump line parse error
#[derive(Error, Debug)]
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

/// Recorded CAN frame.
/// This corresponds to the information in a line from the candump log.
#[derive(Debug, Clone)]
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
        write!(
            f,
            "({:.6}) {} {:03X}",
            1.0e-6 * self.t_us as f64,
            self.device,
            self.frame.raw_id()
        )?;

        use CanAnyFrame::*;
        match self.frame {
            Remote(frame) if frame.len() == 0 => f.write_str("#R"),
            Remote(frame) => write!(f, "#R{}", frame.dlc()),
            Error(_frame) => f.write_str(""),
            Normal(frame) => {
                let mut parts = frame.data().iter().map(|v| format!("{:02X}", v));
                write!(f, "#{}", parts.join(""))
            }
            Fd(frame) => {
                let mut parts = frame.data().iter().map(|v| format!("{:02X}", v));
                write!(f, "##{}", parts.join(""))
            }
        }
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
    pub fn records(&mut self) -> CanDumpRecords<R> {
        CanDumpRecords { src: self }
    }

    /// Advance state, returning next record.
    pub fn next_record(&mut self) -> Result<Option<CanDumpRecord>, ParseError> {
        self.buf.clear();
        let nread = self.rdr.read_line(&mut self.buf)?;

        // reached EOF
        if nread == 0 {
            return Ok(None);
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
                let num = num
                    .parse::<u64>()
                    .map_err(|_| ParseError::InvalidTimestamp)?;
                let mant = mant
                    .parse::<u64>()
                    .map_err(|_| ParseError::InvalidTimestamp)?;
                num.saturating_mul(1_000_000).saturating_add(mant)
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

        // Parse the CAN ID
        let can_id = canid_t::from_str_radix(can_id_str, 16)
            .ok()
            .and_then(id_from_raw)
            .ok_or(ParseError::InvalidCanFrame)?;

        // Determine frame type (FD or classical) and skip separator(s)
        // Remember...
        //   CAN FD: "<canid>##<flags>[data]"
        //   Remote: "<canid>#R[len]"
        //   Data;   "<canid>#[data]"

        let frame: CanAnyFrame = if can_data.starts_with('#') {
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
            let rlen = can_data.parse::<usize>().unwrap_or(0);
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
            &[0x0, 0x011, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB]
        );
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
