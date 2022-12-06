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

use std::{fs, io, path};
use hex::FromHex;
use crate::{CanNormalFrame, CanFdFrame, frame::{CANFD_BRS, CANFD_ESI}};

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
    pub fn from_reader(rdr: R) -> Reader<io::BufReader<R>> {
        Reader {
            rdr: io::BufReader::new(rdr),
            line_buf: Vec::new(),
        }
    }
}

impl Reader<fs::File> {
    pub fn from_file<P: AsRef<path::Path>>(path: P) -> io::Result<Reader<io::BufReader<fs::File>>> {
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
    pub t_us: u64,
    pub device: &'a str,
    pub frame: super::CanAnyFrame,
}

#[derive(Debug)]
/// candump line parse error
pub enum ParseError {
    Io(io::Error),
    UnexpectedEndOfLine,
    InvalidTimestamp,
    InvalidDeviceName,
    InvalidCanFrame,
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
        let dot = inner.iter()
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

        let sep_idx =
            can_raw.iter().position(|&c| c == b'#').ok_or(ParseError::InvalidCanFrame)?;
        let (can_id, mut can_data) = can_raw.split_at(sep_idx);

        // determine frame type (FD or classical) and skip separator(s)
        let mut fd_flags: u8 = 0;
        let is_fd_frame = if let Some(&b'#') = can_data.get(1) {
            fd_flags = can_data[2];
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

        let rtr = b"R" == can_data;

        let data = if rtr {
            Vec::new()
        } else {
            Vec::from_hex(&can_data).map_err(|_| ParseError::InvalidCanFrame)?
        };
        let frame: super::CanAnyFrame = if is_fd_frame {
            CanFdFrame::init(
                parse_raw(can_id, 16).ok_or(ParseError::InvalidCanFrame)? as u32,
                &data,
                false,  // TODO: is extended?
                // FIXME: how are error frames saved?
                false,
                fd_flags & CANFD_BRS == CANFD_BRS,
                fd_flags & CANFD_ESI == CANFD_ESI
            )
            .map(|frame| super::CanAnyFrame::Fd(frame))
        } else {
            CanNormalFrame::init(
                parse_raw(can_id, 16).ok_or(ParseError::InvalidCanFrame)? as u32,
                &data,
                false,  // TODO: is extended?
                rtr,
                false
            )
            .map(|frame| super::CanAnyFrame::Normal(frame))
        }?;

        Ok(Some(CanDumpRecord {
            t_us,
            device,
            frame,
        }))
    }
}

impl<'a, R: io::Read> Iterator for CanDumpRecords<'a, io::BufReader<R>> {
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
    use crate::{Frame, CanAnyFrame, CanNormalFrame as CanFrame, util::hal_id_to_raw};
    use embedded_hal::can::Frame as EmbeddedFrame;

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
                assert_eq!(hal_id_to_raw(rec1.frame.id()), 0x080);
                assert_eq!(frame.is_rtr(), false);
                assert_eq!(frame.is_error(), false);
                assert_eq!(frame.is_extended(), false);
                assert_eq!(frame.data(), &[]);
            }
            else {
                panic!("Expected Normal frame, got FD");
            }
        }

        {
            let rec2 = reader.next_record().unwrap().unwrap();
            assert_eq!(rec2.t_us, 1469439874299654);
            assert_eq!(rec2.device, "can1");

            if let CanAnyFrame::Normal(frame) = rec2.frame {
                assert_eq!(hal_id_to_raw(rec2.frame.id()), 0x701);
                assert_eq!(frame.is_rtr(), false);
                assert_eq!(frame.is_error(), false);
                assert_eq!(frame.is_extended(), false);
                assert_eq!(frame.data(), &[0x7F]);
            }
            else {
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
                assert_eq!(frame.id(), 0x080);
                assert_eq!(frame.is_remote_frame(), false);
                assert_eq!(frame.is_error(), false);
                assert_eq!(frame.is_extended(), false);
                assert_eq!(frame.is_brs(), false);
                assert_eq!(frame.is_esi(), false);
                assert_eq!(frame.data(), &[]);
            }
            else {
                panic!("Expected FD frame, got Normal");
            }
        }

        {
            let rec2 = reader.next_record().unwrap().unwrap();
            assert_eq!(rec2.t_us, 1469439874299654);
            assert_eq!(rec2.device, "can1");
            if let CanAnyFrame::Fd(frame) = rec2.frame {
                assert_eq!(frame.id(), 0x701);
                assert_eq!(frame.is_remote_frame(), false);
                assert_eq!(frame.is_error(), false);
                assert_eq!(frame.is_extended(), false);
                assert_eq!(frame.is_brs(), true);
                assert_eq!(frame.is_esi(), false);
                assert_eq!(frame.data(), &[0x7F]);
            }
            else {
                panic!("Expected FD frame, got Normal");
            }
        }

        assert!(reader.next_record().unwrap().is_none());
    }
}
