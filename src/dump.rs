use std::{fs, io, path};
use std::str::FromStr;

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
        Ok(Reader::from_reader(try!(fs::File::open(path))))
    }
}

pub struct CanDumpRecords<'a, R: 'a> {
    src: &'a mut Reader<R>,
}

pub struct CanDumpRecord<'a> {
    pub t_us: u64,
    pub device: &'a str,
    pub can_raw: &'a [u8],
}

pub enum ParseError {
    Io(io::Error),
    UnexpectedEndOfLine,
    InvalidTimestamp,
    InvalidDeviceName,
}

impl From<io::Error> for ParseError {
    fn from(e: io::Error) -> ParseError {
        ParseError::Io(e)
    }
}

impl<R: io::BufRead> Reader<R> {
    pub fn records<'a>(&'a mut self) -> CanDumpRecords<'a, R> {
        CanDumpRecords { src: self }
    }

    pub fn next_record(&mut self) -> Result<Option<CanDumpRecord>, ParseError> {
        let bytes_read = try!(self.rdr.read_until(b'\n', &mut self.line_buf));

        // reached EOF
        if bytes_read == 0 {
            return Ok(None);
        }

        let mut field_iter = self.line_buf.split(|&c| c == b' ');

        // parse time field
        let f = try!(field_iter.next().ok_or(ParseError::UnexpectedEndOfLine));

        if f.len() < 3 || f[0] != b'(' || f[f.len() - 1] != b')' {
            return Err(ParseError::InvalidTimestamp);
        }

        // split at dot, read both parts
        let dot = try!(f.iter()
            .position(|&c| c == b'.')
            .ok_or(ParseError::InvalidTimestamp));
        let (num, mant) = f.split_at(dot);

        // parse number and multiply
        let n_num: u64 = try!(::std::str::from_utf8(num)
            .ok()
            .and_then(|s| u64::from_str(s).ok())
            .ok_or(ParseError::InvalidTimestamp));
        let n_mant: u64 = try!(::std::str::from_utf8(mant)
            .ok()
            .and_then(|s| u64::from_str(s).ok())
            .ok_or(ParseError::InvalidTimestamp));
        let t_us = n_num.saturating_mul(1_000_000).saturating_add(n_mant);

        let f = try!(field_iter.next().ok_or(ParseError::UnexpectedEndOfLine));
        let device = try!(::std::str::from_utf8(f).map_err(|_| ParseError::InvalidDeviceName));

        let can_raw = try!(field_iter.next().ok_or(ParseError::UnexpectedEndOfLine));

        Ok(Some(CanDumpRecord {
            t_us: t_us,
            device: device,
            can_raw: can_raw,
        }))
    }
}

impl<'a, R: io::Read> Iterator for CanDumpRecords<'a, io::BufReader<R>> {
    type Item = Result<(u64, ()), ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        // lift Option:
        match self.src.next_record() {
            Ok(Some(CanDumpRecord { t_us, .. })) => Some(Ok((t_us, ()))),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}
