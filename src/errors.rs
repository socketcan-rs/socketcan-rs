// socketcan/src/errors.rs
//
// Implements errors for Rust SocketCAN library on Linux.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! CAN bus errors.
//!
//! Most information about the errors on the CANbus are determined from an
//! error frame. To receive them, the error mask must be set on the socket
//! for the types of errors that the application would like to receive.
//!
//! See [RAW Socket Option CAN_RAW_ERR_FILTER](https://docs.kernel.org/networking/can.html#raw-socket-option-can-raw-err-filter)
//!
//! # Layout of an error frame
//!
//! The general classes of error are encoded as bits in the error field of
//! the CAN ID of an error frame. Several classes can be — and routinely
//! are — set at once. Most classes point at a data byte holding further
//! detail:
//!
//! ```text
//! TX Timeout         (0x001)
//! Lost Arbitration   (0x002) => data[0]   bit number, 0 = unspecified
//! Controller Problem (0x004) => data[1]   BITFIELD
//! Protocol Violation (0x008) => data[2]   BITFIELD  (type)
//!                               data[3]   scalar    (location)
//! Transceiver Status (0x010) => data[4]   two nibbles: CANL | CANH
//! No ACK             (0x020)
//! Bus Off            (0x040)
//! Bus Error          (0x080)
//! Restarted          (0x100)
//! Error Counters     (0x200) => data[6]   TX error counter
//!                               data[7]   RX error counter
//!
//! data[5] is reserved by the kernel and is never decoded here.
//! ```
//!
//! # Multiplicity
//!
//! A single error frame can describe **many** distinct error conditions, at
//! two independent levels:
//!
//! 1. Several class bits can be set in the CAN ID at once. `CAN_ERR_CRTL |
//!    CAN_ERR_CNT` accompanies essentially every controller state change,
//!    and drivers such as `sja1000` can set five classes on one frame.
//! 2. `data[1]` and `data[2]` are themselves **bitfields**, so one class can
//!    describe several simultaneous conditions. The kernel's shared
//!    `can_change_state()` helper ORs both the TX and RX state codes into
//!    `data[1]` whenever the two states are equal, so `data[1] = 0x0C`
//!    (`RX_WARNING | TX_WARNING`) is the normal encoding of a symmetric
//!    warning transition. `data[4]` is similarly split into two independent
//!    nibbles for the CAN High and CAN Low lines.
//!
//! Decoding therefore yields a *collection* — [`CanErrors`] — which is
//! non-empty by construction and preserves every condition the frame
//! reported. Entries appear in a documented order: class bits ascending,
//! and within a class byte, least-significant bit first.
//!
//! All of this error information is not well documented, but can be
//! extracted from the Linux kernel header file:
//! [linux/can/error.h](https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/can/error.h)
//!

use crate::{CanErrorFrame, EmbeddedFrame, Frame};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::{convert::TryFrom, error, fmt, io};
use thiserror::Error;

/// The error class bits that can appear in the CAN ID of an error frame.
///
/// Several of these are routinely set at once; see the [module
/// documentation](self). Re-exported so that constructing or inspecting an
/// error frame does not require a direct `libc` dependency.
pub use libc::{
    CAN_ERR_ACK, CAN_ERR_BUSERROR, CAN_ERR_BUSOFF, CAN_ERR_CNT, CAN_ERR_CRTL, CAN_ERR_LOSTARB,
    CAN_ERR_PROT, CAN_ERR_RESTARTED, CAN_ERR_TRX, CAN_ERR_TX_TIMEOUT,
};

/// The error counter value at which a controller enters the "error warning"
/// state. Compare against [`CanError::Counters`].
pub use libc::CAN_ERROR_WARNING_THRESHOLD;

/// The error counter value at which a controller enters the "error passive"
/// state. Compare against [`CanError::Counters`].
pub use libc::CAN_ERROR_PASSIVE_THRESHOLD;

/// The error counter value at which a controller goes bus-off.
/// Compare against [`CanError::Counters`].
pub use libc::CAN_BUS_OFF_THRESHOLD;

/// Mask of every error class bit this crate knows how to decode.
const KNOWN_ERR_CLASSES: u32 = CAN_ERR_TX_TIMEOUT
    | CAN_ERR_LOSTARB
    | CAN_ERR_CRTL
    | CAN_ERR_PROT
    | CAN_ERR_TRX
    | CAN_ERR_ACK
    | CAN_ERR_BUSOFF
    | CAN_ERR_BUSERROR
    | CAN_ERR_RESTARTED
    | CAN_ERR_CNT;

// ===== Composite Error for the crate =====

/// Composite SocketCAN error.
///
/// This can be any of the underlying errors from this library. The two main
/// error sources are either CAN errors coming in through received error
/// frames or from typical system I/O errors.
#[derive(Error, Debug)]
#[cfg_attr(feature = "serde", derive(Deserialize), serde(from = "ErrorRepr"))]
pub enum Error {
    /// One or more CANbus errors, usually from an error frame
    #[error(transparent)]
    Can(#[from] CanErrors),
    /// An I/O Error
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl embedded_can::Error for Error {
    fn kind(&self) -> embedded_can::ErrorKind {
        match self {
            Error::Can(errs) => errs.kind(),
            _ => embedded_can::ErrorKind::Other,
        }
    }
}

impl From<CanError> for Error {
    /// Wraps a single CAN error, promoting it to a one-element collection.
    fn from(err: CanError) -> Self {
        Error::Can(CanErrors::from_single(err))
    }
}

impl From<CanErrorFrame> for Error {
    fn from(frame: CanErrorFrame) -> Self {
        Error::Can(CanErrors::from(frame))
    }
}

impl From<io::ErrorKind> for Error {
    /// Creates an Io error straight from an io::ErrorKind
    fn from(kind: io::ErrorKind) -> Self {
        Self::from(io::Error::from(kind))
    }
}

#[cfg(feature = "netlink")]
impl<T, P> From<neli::err::RouterError<T, P>> for Error
where
    T: neli::consts::nl::NlType,
    P: fmt::Debug,
{
    /// Wraps a netlink error as an [`io::Error`] of kind `Other`, preserving
    /// the underlying description. Lets callers `?` netlink results across
    /// module boundaries into the crate-level [`enum@Error`].
    fn from(e: neli::err::RouterError<T, P>) -> Error {
        Self::Io(io::Error::other(e.to_string()))
    }
}

#[cfg(feature = "dump")]
impl From<crate::dump::ParseError> for Error {
    /// Maps a [`ParseError`](crate::dump::ParseError) into an [`io::Error`]
    /// of kind `InvalidData`, preserving the description. Lets callers `?`
    /// dump-parsing results into the crate-level [`enum@Error`].
    fn from(e: crate::dump::ParseError) -> Error {
        use crate::dump::ParseError;
        match e {
            ParseError::Io(io_err) => Self::Io(io_err),
            other => Self::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                other.to_string(),
            )),
        }
    }
}

/// A result that can derive from any of the CAN errors.
pub type Result<T> = std::result::Result<T, Error>;

/// An I/O specific error
pub type IoError = io::Error;

/// A kind of I/O error
pub type IoErrorKind = io::ErrorKind;

/// An I/O specific result
pub type IoResult<T> = io::Result<T>;

// ===== CanErrors =====

/// One or more CAN errors observed in a single error frame.
///
/// Linux SocketCAN error frames can report multiple distinct error
/// conditions simultaneously — most commonly a controller state change
/// together with the current TX/RX error counter values, or a bus error
/// annotated with several protocol violation types and a location. See the
/// [module documentation](self) for the two levels at which this
/// multiplicity arises.
///
/// `CanErrors` is **non-empty by construction**: there is always a
/// [`first()`](Self::first) error. A frame with no recognisable error bits
/// decodes to a single [`CanError::Unknown`] rather than an empty
/// collection.
///
/// # Ordering
///
/// Entries appear in a stable, documented order:
///
/// - error classes in ascending numeric order of their CAN ID bit, i.e.
///   TX timeout, lost arbitration, controller problem, protocol violation,
///   transceiver status, no-ACK, bus off, bus error, restarted, counters
/// - within `data[1]` and `data[2]`, least-significant bit first
/// - within a protocol violation, all types precede any location decoding
///   failure
/// - within a transceiver status, CAN High precedes CAN Low
/// - any unrecognised class bits produce a trailing [`CanError::Unknown`]
///
/// # Implementation
///
/// This is slightly optimized for the single error case: No allocation is
/// done if there is only one error.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(into = "Vec<CanError>", try_from = "Vec<CanError>")
)]
pub struct CanErrors {
    first: CanError,
    rest: Vec<CanError>,
}

impl CanErrors {
    /// Creates a collection from a first error plus any number of
    /// additional ones.
    pub fn new(first: CanError, rest: impl IntoIterator<Item = CanError>) -> Self {
        Self {
            first,
            rest: rest.into_iter().collect(),
        }
    }

    /// Creates a collection holding exactly one error.
    ///
    /// This does not allocate.
    pub fn from_single(err: CanError) -> Self {
        Self {
            first: err,
            rest: Vec::new(),
        }
    }

    /// Creates a collection from an iterator, returning `None` if it is
    /// empty.
    ///
    /// Prefer [`from_single()`](Self::from_single) or [`new()`](Self::new) when the
    /// non-emptiness is already known statically.
    pub fn from_iter_checked(errs: impl IntoIterator<Item = CanError>) -> Option<Self> {
        let mut it = errs.into_iter();
        let first = it.next()?;
        Some(Self {
            first,
            rest: it.collect(),
        })
    }

    /// Gets the first error.
    ///
    /// This is never `None`: the type is non-empty by construction. For a
    /// frame that set several class bits, this is the one belonging to the
    /// lowest-numbered class bit.
    pub fn first(&self) -> &CanError {
        &self.first
    }

    /// Gets the last error.
    pub fn last(&self) -> &CanError {
        self.rest.last().unwrap_or(&self.first)
    }

    /// The number of errors reported. Always at least one.
    pub fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// Always `false`; the collection is non-empty by construction.
    ///
    /// Provided only because clippy expects `is_empty` alongside `len`.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Determines whether this holds exactly one error.
    ///
    /// Note that multi-entry collections are *common*, not exceptional: any
    /// controller state change reports `CAN_ERR_CRTL | CAN_ERR_CNT`, which
    /// is at least two entries. Do not treat the single case as the norm.
    pub fn is_single(&self) -> bool {
        self.rest.is_empty()
    }

    /// An iterator over the errors, in the order documented on the type.
    pub fn iter(&self) -> impl Iterator<Item = &CanError> + '_ {
        std::iter::once(&self.first).chain(self.rest.iter())
    }

    /// Determines whether any of the errors maps to the given
    /// [`embedded_can::ErrorKind`].
    pub fn contains_kind(&self, kind: embedded_can::ErrorKind) -> bool {
        use embedded_can::Error as _;
        self.iter().any(|e| e.kind() == kind)
    }
}

impl From<CanError> for CanErrors {
    fn from(err: CanError) -> Self {
        Self::from_single(err)
    }
}

impl IntoIterator for CanErrors {
    type Item = CanError;
    type IntoIter = std::iter::Chain<std::iter::Once<CanError>, std::vec::IntoIter<CanError>>;

    fn into_iter(self) -> Self::IntoIter {
        std::iter::once(self.first).chain(self.rest)
    }
}

impl<'a> IntoIterator for &'a CanErrors {
    type Item = &'a CanError;
    type IntoIter = std::iter::Chain<std::iter::Once<&'a CanError>, std::slice::Iter<'a, CanError>>;

    fn into_iter(self) -> Self::IntoIter {
        std::iter::once(&self.first).chain(self.rest.iter())
    }
}

impl error::Error for CanErrors {}

impl fmt::Display for CanErrors {
    /// Renders the errors as a single line.
    ///
    /// A lone error renders exactly as its own `Display`. Multiple errors
    /// are joined with "; ". Consecutive protocol violations that share a
    /// location are grouped, so a frame reporting several violation types
    /// at one location reads as
    /// `protocol violation at CRC sequence: frame format error, bit stuffing error`
    /// rather than repeating the location for each type.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut first_out = true;
        let mut pending: Option<(Location, Vec<ViolationType>)> = None;

        // Flushes a grouped run of protocol violations.
        fn flush(
            f: &mut fmt::Formatter,
            first_out: &mut bool,
            pending: &mut Option<(Location, Vec<ViolationType>)>,
        ) -> fmt::Result {
            if let Some((location, types)) = pending.take() {
                if !*first_out {
                    write!(f, "; ")?;
                }
                *first_out = false;
                write!(f, "protocol violation at {}: ", location)?;
                for (i, t) in types.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", t)?;
                }
            }
            Ok(())
        }

        for err in self.iter() {
            if let CanError::ProtocolViolation { vtype, location } = err {
                match pending.as_mut() {
                    Some((loc, types)) if *loc == *location => {
                        types.push(*vtype);
                        continue;
                    }
                    _ => {
                        flush(f, &mut first_out, &mut pending)?;
                        pending = Some((*location, vec![*vtype]));
                        continue;
                    }
                }
            }
            flush(f, &mut first_out, &mut pending)?;
            if !first_out {
                write!(f, "; ")?;
            }
            first_out = false;
            write!(f, "{}", err)?;
        }
        flush(f, &mut first_out, &mut pending)
    }
}

impl embedded_can::Error for CanErrors {
    /// Reports the most specific error kind present.
    ///
    /// Scans the entries in order and returns the first kind that is not
    /// [`ErrorKind::Other`](embedded_can::ErrorKind::Other), falling back to
    /// `Other` when every entry is unspecific. The scan is over *kinds*, not
    /// over error classes in declaration order — a frame carrying both a
    /// controller warning and a missing ACK reports `Acknowledge`, since the
    /// warning maps only to `Other`.
    fn kind(&self) -> embedded_can::ErrorKind {
        use embedded_can::ErrorKind;
        self.iter()
            .map(|e| e.kind())
            .find(|k| *k != ErrorKind::Other)
            .unwrap_or(ErrorKind::Other)
    }
}

impl From<CanErrorFrame> for CanErrors {
    /// Decodes every error condition described by an error frame.
    ///
    /// Walks the class bits of the CAN ID in ascending order, decomposing
    /// the bitfield data bytes so that a class describing several
    /// simultaneous conditions yields one entry per condition.
    fn from(frame: CanErrorFrame) -> Self {
        // Note that the CanErrorFrame is guaranteed to have the full 8-byte
        // data payload.
        let bits = frame.error_bits();
        let data = frame.data();
        let mut errs: Vec<CanError> = Vec::new();

        if bits & CAN_ERR_TX_TIMEOUT != 0 {
            errs.push(CanError::TransmitTimeout);
        }
        if bits & CAN_ERR_LOSTARB != 0 {
            errs.push(CanError::LostArbitration(data[0]));
        }
        if bits & CAN_ERR_CRTL != 0 {
            push_ctrl(&mut errs, data[1]);
        }
        if bits & CAN_ERR_PROT != 0 {
            push_prot(&mut errs, data[2], data[3]);
        }
        if bits & CAN_ERR_TRX != 0 {
            push_trx(&mut errs, data[4]);
        }
        if bits & CAN_ERR_ACK != 0 {
            errs.push(CanError::NoAck);
        }
        if bits & CAN_ERR_BUSOFF != 0 {
            errs.push(CanError::BusOff);
        }
        if bits & CAN_ERR_BUSERROR != 0 {
            errs.push(CanError::BusError);
        }
        if bits & CAN_ERR_RESTARTED != 0 {
            errs.push(CanError::Restarted);
        }
        // Strictly gated on the flag. The kernel leaves data[6..7] undefined
        // when CAN_ERR_CNT is clear; can-utils prints them anyway, but that
        // is a display convenience, not a decoding rule.
        if bits & CAN_ERR_CNT != 0 {
            errs.push(CanError::Counters {
                tx: data[6],
                rx: data[7],
            });
        }

        // Any class bits we do not recognise are reported as a single
        // trailing entry carrying just those bits.
        let unknown = bits & !KNOWN_ERR_CLASSES;
        if unknown != 0 {
            errs.push(CanError::Unknown(unknown));
        }

        // A frame with no class bits at all is malformed; report it rather
        // than violating the non-empty invariant.
        Self::from_iter_checked(errs).unwrap_or_else(|| Self::from_single(CanError::Unknown(0)))
    }
}

/// Decomposes the controller-problem bitfield in `data[1]`.
///
/// Pushes one entry per set bit, or a single `Unspecified` for a zero byte.
/// Guaranteed to push at least one entry.
fn push_ctrl(errs: &mut Vec<CanError>, byte: u8) {
    // `data[1] == 0` is CAN_ERR_CRTL_UNSPEC, a legitimate value. It must be
    // handled before the bit walk, because `byte & 0 != 0` is never true and
    // a naive loop would report a valid frame as a decoding failure.
    if byte == 0 {
        errs.push(CanError::ControllerProblem(ControllerProblem::Unspecified));
        return;
    }
    let mut matched = 0u8;
    for prob in ControllerProblem::ALL {
        let bit = prob as u8;
        if byte & bit != 0 {
            errs.push(CanError::ControllerProblem(prob));
            matched |= bit;
        }
    }
    if byte & !matched != 0 {
        errs.push(CanError::DecodingFailure(
            CanErrorDecodingFailure::InvalidControllerProblem,
        ));
    }
}

/// Decomposes the protocol-violation bitfield in `data[2]` and pairs each
/// type with the single location in `data[3]`.
///
/// Guaranteed to push at least one entry.
fn push_prot(errs: &mut Vec<CanError>, types: u8, loc: u8) {
    let location = Location::from_raw(loc);

    // As with data[1], zero means "unspecified" and must bypass the walk.
    if types == 0 {
        errs.push(CanError::ProtocolViolation {
            vtype: ViolationType::Unspecified,
            location,
        });
        return;
    }
    // Every bit of data[2] is claimed by a known violation type, so unlike
    // data[1] there is no unmatched-bits case to report here.
    for vtype in ViolationType::ALL {
        if types & (vtype as u8) != 0 {
            errs.push(CanError::ProtocolViolation { vtype, location });
        }
    }
}

/// Decodes the transceiver status byte `data[4]`.
///
/// `data[4]` is **two independent nibbles**: the low nibble describes the
/// CAN High line and the high nibble the CAN Low line, so a fault on both
/// lines is reported as a single byte with both halves set (the kernel's
/// `etas_es58x` driver emits `0x44` for a lost connection on either line).
/// Each non-zero half yields its own entry.
///
/// Guaranteed to push at least one entry.
fn push_trx(errs: &mut Vec<CanError>, byte: u8) {
    if byte == 0 {
        errs.push(CanError::TransceiverError(TransceiverError::Unspecified));
        return;
    }
    // The enum discriminants are already nibble-aligned: the CanHigh*
    // values occupy 0x04..=0x07 and the CanLow* values 0x40..=0x80, so each
    // masked half decodes directly.
    for half in [byte & 0x0F, byte & 0xF0] {
        if half == 0 {
            continue;
        }
        match TransceiverError::try_from(half) {
            Ok(e) => errs.push(CanError::TransceiverError(e)),
            Err(e) => errs.push(CanError::DecodingFailure(e)),
        }
    }
}

/////////////////////////////////////////////////////////////////////////////
// serde support for the composite error and the error collection

/// Serialized form of [`enum@Error`].
///
/// The `Can` half round-trips exactly. The `Io` half cannot: `io::Error`
/// implements neither serde trait and may carry an OS errno or a boxed source,
/// so it is reduced to its kind and message. See [`ErrorRepr::Io`].
#[cfg(feature = "serde")]
#[derive(Debug, Serialize, Deserialize)]
pub enum ErrorRepr {
    /// One or more CAN errors
    Can(CanErrors),
    /// An I/O error, reduced to a kind name and a message.
    ///
    /// This conversion is **lossy**: after a round trip
    /// [`io::Error::raw_os_error()`] returns `None`, any `source()` chain is
    /// gone, and a `kind` name that the reading version does not recognise
    /// degrades to [`io::ErrorKind::Other`].
    Io {
        /// The [`io::ErrorKind`], by name
        kind: String,
        /// The original error's `Display` text
        message: String,
    },
}

/// Maps an [`io::ErrorKind`] to a stable name.
///
/// `io::ErrorKind` is `#[non_exhaustive]` and has no stable string form of its
/// own, so this covers the kinds this crate can plausibly produce and falls
/// back to `"Other"`. Round-tripping through [`io_kind_from_name`] is therefore
/// not total, which is documented on [`ErrorRepr::Io`].
///
/// Note that some errnos map to kinds that are not nameable at all — `ENODEV`
/// becomes the unstable `ErrorKind::Uncategorized`, for instance — so those
/// necessarily arrive back as `Other`.
#[cfg(feature = "serde")]
pub(crate) fn io_kind_name(kind: io::ErrorKind) -> &'static str {
    use io::ErrorKind::*;
    match kind {
        NotFound => "NotFound",
        PermissionDenied => "PermissionDenied",
        ConnectionRefused => "ConnectionRefused",
        ConnectionReset => "ConnectionReset",
        ConnectionAborted => "ConnectionAborted",
        NotConnected => "NotConnected",
        NetworkDown => "NetworkDown",
        NetworkUnreachable => "NetworkUnreachable",
        HostUnreachable => "HostUnreachable",
        ResourceBusy => "ResourceBusy",
        AddrInUse => "AddrInUse",
        AddrNotAvailable => "AddrNotAvailable",
        BrokenPipe => "BrokenPipe",
        AlreadyExists => "AlreadyExists",
        WouldBlock => "WouldBlock",
        InvalidInput => "InvalidInput",
        InvalidData => "InvalidData",
        TimedOut => "TimedOut",
        WriteZero => "WriteZero",
        Interrupted => "Interrupted",
        Unsupported => "Unsupported",
        UnexpectedEof => "UnexpectedEof",
        OutOfMemory => "OutOfMemory",
        _ => "Other",
    }
}

/// The inverse of [`io_kind_name`], falling back to
/// [`io::ErrorKind::Other`] for anything unrecognised.
///
/// The fallback is deliberate: a value written by a newer version of this
/// crate must still deserialize in an older one.
#[cfg(feature = "serde")]
pub(crate) fn io_kind_from_name(name: &str) -> io::ErrorKind {
    use io::ErrorKind::*;
    match name {
        "NotFound" => NotFound,
        "PermissionDenied" => PermissionDenied,
        "ConnectionRefused" => ConnectionRefused,
        "ConnectionReset" => ConnectionReset,
        "ConnectionAborted" => ConnectionAborted,
        "NotConnected" => NotConnected,
        "NetworkDown" => NetworkDown,
        "NetworkUnreachable" => NetworkUnreachable,
        "HostUnreachable" => HostUnreachable,
        "ResourceBusy" => ResourceBusy,
        "AddrInUse" => AddrInUse,
        "AddrNotAvailable" => AddrNotAvailable,
        "BrokenPipe" => BrokenPipe,
        "AlreadyExists" => AlreadyExists,
        "WouldBlock" => WouldBlock,
        "InvalidInput" => InvalidInput,
        "InvalidData" => InvalidData,
        "TimedOut" => TimedOut,
        "WriteZero" => WriteZero,
        "Interrupted" => Interrupted,
        "Unsupported" => Unsupported,
        "UnexpectedEof" => UnexpectedEof,
        "OutOfMemory" => OutOfMemory,
        _ => Other,
    }
}

/// Hand-written because `serde(into = ...)` requires `Clone`, and
/// [`enum@Error`] cannot be `Clone`: `io::Error` is not.
///
/// This clones the [`CanErrors`] to build the repr. Serialization is not a hot
/// path, so the allocation is not worth avoiding with a parallel borrowing
/// repr that would have to be kept in sync by hand.
#[cfg(feature = "serde")]
impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> std::result::Result<S::Ok, S::Error> {
        let repr = match self {
            Error::Can(errs) => ErrorRepr::Can(errs.clone()),
            Error::Io(e) => ErrorRepr::Io {
                kind: io_kind_name(e.kind()).to_string(),
                message: e.to_string(),
            },
        };
        repr.serialize(ser)
    }
}

#[cfg(feature = "serde")]
impl From<ErrorRepr> for Error {
    fn from(repr: ErrorRepr) -> Self {
        match repr {
            ErrorRepr::Can(errs) => Self::Can(errs),
            ErrorRepr::Io { kind, message } => {
                Self::Io(io::Error::new(io_kind_from_name(&kind), message))
            }
        }
    }
}

#[cfg(feature = "serde")]
impl From<CanErrors> for Vec<CanError> {
    fn from(errs: CanErrors) -> Self {
        errs.into_iter().collect()
    }
}

#[cfg(feature = "serde")]
impl TryFrom<Vec<CanError>> for CanErrors {
    type Error = EmptyCanErrors;

    /// Rebuilds the collection, rejecting an empty sequence.
    ///
    /// This is what keeps the non-empty invariant intact across
    /// deserialization; without it, serde would be a way to construct an
    /// invalid `CanErrors` from outside the crate.
    fn try_from(errs: Vec<CanError>) -> std::result::Result<Self, Self::Error> {
        Self::from_iter_checked(errs).ok_or(EmptyCanErrors)
    }
}

/// Error returned when deserializing a [`CanErrors`] from an empty sequence.
///
/// [`CanErrors`] is non-empty by construction, so an empty input is invalid
/// rather than merely unusual.
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyCanErrors;

#[cfg(feature = "serde")]
impl fmt::Display for EmptyCanErrors {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a CanErrors collection must hold at least one error")
    }
}

#[cfg(feature = "serde")]
impl error::Error for EmptyCanErrors {}

// ===== CanError ====

/// A single CAN bus error condition derived from an error frame.
///
/// An CAN interface device driver can send detailed error information up
/// to the application in an "error frame". These are selectable by the
/// application by applying an error bitmask to the socket to choose which
/// types of errors to receive.
///
/// A single error frame commonly describes several of these at once; the
/// frame as a whole converts to a [`CanErrors`] collection rather than to
/// one of these directly.
///
/// Most error types here correspond to a bit in the error mask of a CAN ID
/// word of an error frame - a frame in which the CAN error flag
/// (`CAN_ERR_FLAG`) is set. But there are additional types to handle any
/// problems decoding the error frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum CanError {
    /// TX timeout (by netdevice driver)
    TransmitTimeout,
    /// Arbitration was lost.
    ///
    /// Contains the bit number after which arbitration was lost. Note that
    /// the kernel uses zero (`CAN_ERR_LOSTARB_UNSPEC`) to mean
    /// *unspecified* rather than literally "bit 0".
    LostArbitration(u8),
    /// Controller problem
    ControllerProblem(ControllerProblem),
    /// Protocol violation at the specified [`Location`].
    ///
    /// A frame reporting several violation types shares one location among
    /// them, so several of these can appear with the same `location`.
    ProtocolViolation {
        /// The type of protocol violation
        vtype: ViolationType,
        /// The location (field or bit) of the violation
        location: Location,
    },
    /// Transceiver error, decoded from `data[4]`.
    ///
    /// The CAN High and CAN Low lines are reported independently, so a
    /// fault on both produces two of these.
    TransceiverError(TransceiverError),
    /// No ACK received for current CAN frame.
    NoAck,
    /// Bus off (due to too many detected errors)
    BusOff,
    /// Bus error (due to too many detected errors)
    BusError,
    /// The bus has been restarted
    Restarted,
    /// The controller's TX and RX error counter values, from a
    /// `CAN_ERR_CNT` frame.
    ///
    /// Compare against [`CAN_ERROR_WARNING_THRESHOLD`],
    /// [`CAN_ERROR_PASSIVE_THRESHOLD`] and [`CAN_BUS_OFF_THRESHOLD`] to
    /// interpret the values.
    Counters {
        /// TX error counter, from `data[6]`
        tx: u8,
        /// RX error counter, from `data[7]`
        rx: u8,
    },
    /// There was an error decoding the error frame
    DecodingFailure(CanErrorDecodingFailure),
    /// Unknown, possibly invalid, error class bits
    Unknown(u32),
}

impl error::Error for CanError {}

impl fmt::Display for CanError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use CanError::*;
        match *self {
            TransmitTimeout => write!(f, "transmission timeout"),
            LostArbitration(n) => write!(f, "arbitration lost after {} bits", n),
            ControllerProblem(e) => write!(f, "controller problem: {}", e),
            ProtocolViolation { vtype, location } => {
                write!(f, "protocol violation at {}: {}", location, vtype)
            }
            TransceiverError(e) => write!(f, "transceiver error: {}", e),
            NoAck => write!(f, "no ack"),
            BusOff => write!(f, "bus off"),
            BusError => write!(f, "bus error"),
            Restarted => write!(f, "restarted"),
            Counters { tx, rx } => write!(f, "error counters: tx={}, rx={}", tx, rx),
            DecodingFailure(err) => write!(f, "decoding failure: {}", err),
            Unknown(err) => write!(f, "unknown error ({:#x})", err),
        }
    }
}

impl embedded_can::Error for CanError {
    fn kind(&self) -> embedded_can::ErrorKind {
        use embedded_can::ErrorKind;
        match *self {
            CanError::ControllerProblem(cp) => {
                use ControllerProblem::*;
                match cp {
                    ReceiveBufferOverflow | TransmitBufferOverflow => ErrorKind::Overrun,
                    _ => ErrorKind::Other,
                }
            }
            CanError::ProtocolViolation { vtype, .. } => {
                use ViolationType::*;
                match vtype {
                    SingleBitError | UnableToSendDominantBit | UnableToSendRecessiveBit => {
                        ErrorKind::Bit
                    }
                    FrameFormatError => ErrorKind::Form,
                    BitStuffingError => ErrorKind::Stuff,
                    _ => ErrorKind::Other,
                }
            }
            CanError::NoAck => ErrorKind::Acknowledge,
            _ => ErrorKind::Other,
        }
    }
}

// ===== ControllerProblem =====

/// Error status of the CAN controller.
///
/// This is derived from `data[1]` of an error frame, which is a **bitfield**
/// — several of these can be reported at once. The kernel's shared
/// `can_change_state()` helper ORs the TX and RX state codes together
/// whenever the two states match, so pairs such as `ReceiveErrorWarning`
/// plus `TransmitErrorWarning` are the normal encoding rather than an
/// anomaly.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ControllerProblem {
    /// unspecified
    Unspecified = 0x00,
    /// RX buffer overflow
    ReceiveBufferOverflow = libc::CAN_ERR_CRTL_RX_OVERFLOW as u8,
    /// TX buffer overflow
    TransmitBufferOverflow = libc::CAN_ERR_CRTL_TX_OVERFLOW as u8,
    /// reached warning level for RX errors
    ReceiveErrorWarning = libc::CAN_ERR_CRTL_RX_WARNING as u8,
    /// reached warning level for TX errors
    TransmitErrorWarning = libc::CAN_ERR_CRTL_TX_WARNING as u8,
    /// reached error passive status RX
    ReceiveErrorPassive = libc::CAN_ERR_CRTL_RX_PASSIVE as u8,
    /// reached error passive status TX
    TransmitErrorPassive = libc::CAN_ERR_CRTL_TX_PASSIVE as u8,
    /// recovered to error active state
    BackToErrorActive = libc::CAN_ERR_CRTL_ACTIVE as u8,
}

impl ControllerProblem {
    /// Every problem that occupies a bit in `data[1]`, least-significant
    /// bit first.
    ///
    /// Deliberately excludes [`Unspecified`](Self::Unspecified), whose
    /// value is zero and so cannot be found by a bit test. A zero `data[1]`
    /// means "unspecified" and is handled before any bit walk.
    pub const ALL: [Self; 7] = [
        Self::ReceiveBufferOverflow,
        Self::TransmitBufferOverflow,
        Self::ReceiveErrorWarning,
        Self::TransmitErrorWarning,
        Self::ReceiveErrorPassive,
        Self::TransmitErrorPassive,
        Self::BackToErrorActive,
    ];
}

impl error::Error for ControllerProblem {}

impl fmt::Display for ControllerProblem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ControllerProblem::*;
        let msg = match *self {
            Unspecified => "unspecified controller problem",
            ReceiveBufferOverflow => "receive buffer overflow",
            TransmitBufferOverflow => "transmit buffer overflow",
            ReceiveErrorWarning => "ERROR WARNING (receive)",
            TransmitErrorWarning => "ERROR WARNING (transmit)",
            ReceiveErrorPassive => "ERROR PASSIVE (receive)",
            TransmitErrorPassive => "ERROR PASSIVE (transmit)",
            BackToErrorActive => "back to ERROR ACTIVE",
        };
        write!(f, "{}", msg)
    }
}

impl TryFrom<u8> for ControllerProblem {
    type Error = CanErrorDecodingFailure;

    /// Decodes a *single* controller problem code.
    ///
    /// Note that `data[1]` is a bitfield and may hold several of these at
    /// once, in which case this rejects the combined value. Use
    /// [`CanErrors::from`] on the frame to decode all of them.
    fn try_from(val: u8) -> std::result::Result<Self, Self::Error> {
        use ControllerProblem::*;
        Ok(match val {
            0x00 => Unspecified,
            0x01 => ReceiveBufferOverflow,
            0x02 => TransmitBufferOverflow,
            0x04 => ReceiveErrorWarning,
            0x08 => TransmitErrorWarning,
            0x10 => ReceiveErrorPassive,
            0x20 => TransmitErrorPassive,
            0x40 => BackToErrorActive,
            _ => return Err(CanErrorDecodingFailure::InvalidControllerProblem),
        })
    }
}

// ===== ViolationType =====

/// The type of protocol violation error.
///
/// This is derived from `data[2]` of an error frame, which is a **bitfield**
/// — several of these can be reported at once. Note that
/// [`TransmissionError`](Self::TransmissionError) (`CAN_ERR_PROT_TX`) is
/// really a direction annotation meaning "the error occurred while
/// transmitting"; drivers OR it alongside a specific type rather than
/// reporting it alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ViolationType {
    /// Unspecified Violation
    Unspecified = 0x00,
    /// Single Bit Error
    SingleBitError = libc::CAN_ERR_PROT_BIT as u8,
    /// Frame formatting error
    FrameFormatError = libc::CAN_ERR_PROT_FORM as u8,
    /// Bit stuffing error
    BitStuffingError = libc::CAN_ERR_PROT_STUFF as u8,
    /// A dominant bit was sent, but not received
    UnableToSendDominantBit = libc::CAN_ERR_PROT_BIT0 as u8,
    /// A recessive bit was sent, but not received
    UnableToSendRecessiveBit = libc::CAN_ERR_PROT_BIT1 as u8,
    /// Bus overloaded
    BusOverload = libc::CAN_ERR_PROT_OVERLOAD as u8,
    /// Active error announcement
    Active = libc::CAN_ERR_PROT_ACTIVE as u8,
    /// Error occurred on transmission
    TransmissionError = libc::CAN_ERR_PROT_TX as u8,
}

impl ViolationType {
    /// Every violation type that occupies a bit in `data[2]`,
    /// least-significant bit first.
    ///
    /// Deliberately excludes [`Unspecified`](Self::Unspecified), whose
    /// value is zero and so cannot be found by a bit test. Between them
    /// these eight claim every bit of the byte, so a bit walk over `ALL`
    /// can never leave an unmatched bit behind.
    pub const ALL: [Self; 8] = [
        Self::SingleBitError,
        Self::FrameFormatError,
        Self::BitStuffingError,
        Self::UnableToSendDominantBit,
        Self::UnableToSendRecessiveBit,
        Self::BusOverload,
        Self::Active,
        Self::TransmissionError,
    ];
}

impl error::Error for ViolationType {}

impl fmt::Display for ViolationType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ViolationType::*;
        let msg = match *self {
            Unspecified => "unspecified",
            SingleBitError => "single bit error",
            FrameFormatError => "frame format error",
            BitStuffingError => "bit stuffing error",
            UnableToSendDominantBit => "unable to send dominant bit",
            UnableToSendRecessiveBit => "unable to send recessive bit",
            BusOverload => "bus overload",
            Active => "active error announcement",
            TransmissionError => "error on transmission",
        };
        write!(f, "{}", msg)
    }
}

impl TryFrom<u8> for ViolationType {
    type Error = CanErrorDecodingFailure;

    /// Decodes a *single* violation type code.
    ///
    /// Note that `data[2]` is a bitfield and may hold several of these at
    /// once, in which case this rejects the combined value. Use
    /// [`CanErrors::from`] on the frame to decode all of them.
    fn try_from(val: u8) -> std::result::Result<Self, Self::Error> {
        use ViolationType::*;
        Ok(match val {
            0x00 => Unspecified,
            0x01 => SingleBitError,
            0x02 => FrameFormatError,
            0x04 => BitStuffingError,
            0x08 => UnableToSendDominantBit,
            0x10 => UnableToSendRecessiveBit,
            0x20 => BusOverload,
            0x40 => Active,
            0x80 => TransmissionError,
            _ => return Err(CanErrorDecodingFailure::InvalidViolationType),
        })
    }
}

// ===== Location =====

/// The location of a CANbus protocol violation.
///
/// This describes the position inside a received frame (as in the field
/// or bit) at which an error occurred. It is derived from `data[3]` of an
/// error frame, which — unlike `data[2]` — is a scalar code, not a
/// bitfield.
///
/// # Coverage
///
/// Nineteen of these codes are named in `linux/can/error.h`. A further five
/// (`ActiveErrorFlag`, `TolerateDominantBits`, `PassiveErrorFlag`,
/// `ErrorDelimiter`, `OverloadFlag`) are absent from that header but are
/// genuinely emitted: the `sja1000` driver copies the raw 5-bit error code
/// capture segment straight into `data[3]`, and can-utils names them. Any
/// remaining value decodes to [`Reserved`](Self::Reserved), which keeps the
/// raw byte so nothing is lost and so decoding `data[3]` can never fail.
#[derive(Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Location {
    /// Unspecified
    Unspecified,
    /// Start of frame
    StartOfFrame,
    /// ID bits 28-21 (SFF: 10-3)
    Id2821,
    /// ID bits 20-18 (SFF: 2-0)
    Id2018,
    /// substitute RTR (SFF: RTR)
    SubstituteRtr,
    /// extension of identifier
    IdentifierExtension,
    /// ID bits 17-13
    Id1713,
    /// ID bits 12-5
    Id1205,
    /// ID bits 4-0
    Id0400,
    /// RTR bit
    Rtr,
    /// Reserved bit 1
    Reserved1,
    /// Reserved bit 0
    Reserved0,
    /// Data length
    DataLengthCode,
    /// Data section
    DataSection,
    /// CRC sequence
    CrcSequence,
    /// CRC delimiter
    CrcDelimiter,
    /// ACK slot
    AckSlot,
    /// ACK delimiter
    AckDelimiter,
    /// End-of-frame
    EndOfFrame,
    /// Intermission (between frames)
    Intermission,
    /// Active error flag.
    ///
    /// Not named in `linux/can/error.h`; emitted by controllers that report
    /// the raw error code capture segment.
    ActiveErrorFlag,
    /// Tolerate dominant bits.
    ///
    /// Not named in `linux/can/error.h`; see [`ActiveErrorFlag`](Self::ActiveErrorFlag).
    TolerateDominantBits,
    /// Passive error flag.
    ///
    /// Not named in `linux/can/error.h`; see [`ActiveErrorFlag`](Self::ActiveErrorFlag).
    PassiveErrorFlag,
    /// Error delimiter.
    ///
    /// Not named in `linux/can/error.h`; see [`ActiveErrorFlag`](Self::ActiveErrorFlag).
    ErrorDelimiter,
    /// Overload flag.
    ///
    /// Not named in `linux/can/error.h`; see [`ActiveErrorFlag`](Self::ActiveErrorFlag).
    OverloadFlag,
    /// A `data[3]` value with no known meaning, preserved verbatim.
    Reserved(u8),
}

impl Location {
    /// Decodes the `data[3]` byte of an error frame.
    ///
    /// Total: every one of the 256 possible byte values maps to a variant,
    /// with unknown codes preserved as [`Reserved`](Self::Reserved). This is
    /// why there is no `TryFrom<u8>` — decoding a location cannot fail.
    pub const fn from_raw(val: u8) -> Self {
        use Location::*;
        match val {
            0x00 => Unspecified,
            0x02 => Id2821,
            0x03 => StartOfFrame,
            0x04 => SubstituteRtr,
            0x05 => IdentifierExtension,
            0x06 => Id2018,
            0x07 => Id1713,
            0x08 => CrcSequence,
            0x09 => Reserved0,
            0x0A => DataSection,
            0x0B => DataLengthCode,
            0x0C => Rtr,
            0x0D => Reserved1,
            0x0E => Id0400,
            0x0F => Id1205,
            0x11 => ActiveErrorFlag,
            0x12 => Intermission,
            0x13 => TolerateDominantBits,
            0x16 => PassiveErrorFlag,
            0x17 => ErrorDelimiter,
            0x18 => CrcDelimiter,
            0x19 => AckSlot,
            0x1A => EndOfFrame,
            0x1B => AckDelimiter,
            0x1C => OverloadFlag,
            other => Reserved(other),
        }
    }

    /// The raw `data[3]` byte value for this location.
    ///
    /// Round-trips with [`from_raw()`](Self::from_raw).
    pub const fn as_raw(&self) -> u8 {
        use Location::*;
        match *self {
            Unspecified => 0x00,
            Id2821 => 0x02,
            StartOfFrame => 0x03,
            SubstituteRtr => 0x04,
            IdentifierExtension => 0x05,
            Id2018 => 0x06,
            Id1713 => 0x07,
            CrcSequence => 0x08,
            Reserved0 => 0x09,
            DataSection => 0x0A,
            DataLengthCode => 0x0B,
            Rtr => 0x0C,
            Reserved1 => 0x0D,
            Id0400 => 0x0E,
            Id1205 => 0x0F,
            ActiveErrorFlag => 0x11,
            Intermission => 0x12,
            TolerateDominantBits => 0x13,
            PassiveErrorFlag => 0x16,
            ErrorDelimiter => 0x17,
            CrcDelimiter => 0x18,
            AckSlot => 0x19,
            EndOfFrame => 0x1A,
            AckDelimiter => 0x1B,
            OverloadFlag => 0x1C,
            Reserved(v) => v,
        }
    }
}

impl From<u8> for Location {
    fn from(val: u8) -> Self {
        Self::from_raw(val)
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use Location::*;
        let msg = match *self {
            Unspecified => "unspecified location",
            StartOfFrame => "start of frame",
            Id2821 => "ID, bits 28-21",
            Id2018 => "ID, bits 20-18",
            SubstituteRtr => "substitute RTR bit",
            IdentifierExtension => "ID, extension",
            Id1713 => "ID, bits 17-13",
            Id1205 => "ID, bits 12-05",
            Id0400 => "ID, bits 04-00",
            Rtr => "RTR bit",
            Reserved1 => "reserved bit 1",
            Reserved0 => "reserved bit 0",
            DataLengthCode => "data length code",
            DataSection => "data section",
            CrcSequence => "CRC sequence",
            CrcDelimiter => "CRC delimiter",
            AckSlot => "ACK slot",
            AckDelimiter => "ACK delimiter",
            EndOfFrame => "end of frame",
            Intermission => "intermission",
            ActiveErrorFlag => "active error flag",
            TolerateDominantBits => "tolerate dominant bits",
            PassiveErrorFlag => "passive error flag",
            ErrorDelimiter => "error delimiter",
            OverloadFlag => "overload flag",
            Reserved(v) => return write!(f, "reserved location ({:#04x})", v),
        };
        write!(f, "{}", msg)
    }
}

// ===== TransceiverError =====

/// The error status of the CAN transceiver.
///
/// This is derived from `data[4]` of an error frame. That byte is **two
/// independent nibbles**: the low nibble describes the CAN High line and
/// the high nibble the CAN Low line, so a fault on both lines arrives as a
/// single byte with both halves set and decodes to two of these values.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum TransceiverError {
    /// Unspecified
    Unspecified = 0x00,
    /// CAN High, no wire
    CanHighNoWire = libc::CAN_ERR_TRX_CANH_NO_WIRE as u8,
    /// CAN High, short to BAT
    CanHighShortToBat = libc::CAN_ERR_TRX_CANH_SHORT_TO_BAT as u8,
    /// CAN High, short to VCC
    CanHighShortToVcc = libc::CAN_ERR_TRX_CANH_SHORT_TO_VCC as u8,
    /// CAN High, short to GND
    CanHighShortToGnd = libc::CAN_ERR_TRX_CANH_SHORT_TO_GND as u8,
    /// CAN Low, no wire
    CanLowNoWire = libc::CAN_ERR_TRX_CANL_NO_WIRE as u8,
    /// CAN Low, short to BAT
    CanLowShortToBat = libc::CAN_ERR_TRX_CANL_SHORT_TO_BAT as u8,
    /// CAN Low, short to VCC
    CanLowShortToVcc = libc::CAN_ERR_TRX_CANL_SHORT_TO_VCC as u8,
    /// CAN Low, short to GND
    CanLowShortToGnd = libc::CAN_ERR_TRX_CANL_SHORT_TO_GND as u8,
    /// CAN Low short to CAN High
    CanLowShortToCanHigh = libc::CAN_ERR_TRX_CANL_SHORT_TO_CANH as u8,
}

impl error::Error for TransceiverError {}

impl TryFrom<u8> for TransceiverError {
    type Error = CanErrorDecodingFailure;

    /// Decodes a single transceiver code.
    ///
    /// Expects one nibble's worth of information: either a CAN High code
    /// (`0x00..=0x0F`) or a CAN Low code (`0x00..=0xF0`, low nibble clear).
    /// A byte with both halves set describes two faults and must be split
    /// before calling this.
    fn try_from(val: u8) -> std::result::Result<Self, Self::Error> {
        use TransceiverError::*;
        Ok(match val {
            0x00 => Unspecified,
            0x04 => CanHighNoWire,
            0x05 => CanHighShortToBat,
            0x06 => CanHighShortToVcc,
            0x07 => CanHighShortToGnd,
            0x40 => CanLowNoWire,
            0x50 => CanLowShortToBat,
            0x60 => CanLowShortToVcc,
            0x70 => CanLowShortToGnd,
            0x80 => CanLowShortToCanHigh,
            _ => return Err(CanErrorDecodingFailure::InvalidTransceiverError),
        })
    }
}

impl fmt::Display for TransceiverError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use TransceiverError::*;
        let msg = match *self {
            Unspecified => "unspecified",
            CanHighNoWire => "CAN High, no wire",
            CanHighShortToBat => "CAN High, short to BAT",
            CanHighShortToVcc => "CAN High, short to VCC",
            CanHighShortToGnd => "CAN High, short to GND",
            CanLowNoWire => "CAN Low, no wire",
            CanLowShortToBat => "CAN Low, short to BAT",
            CanLowShortToVcc => "CAN Low, short to VCC",
            CanLowShortToGnd => "CAN Low, short to GND",
            CanLowShortToCanHigh => "CAN Low, short to CAN High",
        };
        write!(f, "{}", msg)
    }
}

/// Get the controller specific error information.
pub trait ControllerSpecificErrorInformation {
    /// Get the controller specific error information.
    fn get_ctrl_err(&self) -> Option<&[u8]>;
}

impl<T: Frame> ControllerSpecificErrorInformation for T {
    /// Get the controller specific error information.
    fn get_ctrl_err(&self) -> Option<&[u8]> {
        let data = self.data();

        if data.len() == 8 {
            Some(&data[5..])
        } else {
            None
        }
    }
}

// ===== CanErrorDecodingFailure =====

/// Error decoding a [`CanError`] from a [`CanErrorFrame`].
///
/// Only two conditions in an error frame are genuinely undecodable, both of
/// them a data byte holding a bit pattern with no defined meaning. Locations
/// (`data[3]`) and protocol violation types (`data[2]`) cannot fail: the
/// former preserves unknown values as [`Location::Reserved`] and the latter
/// has a named type for every bit of the byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum CanErrorDecodingFailure {
    /// `data[1]` had a bit set that no known controller problem claims.
    InvalidControllerProblem,
    /// The type of the ProtocolViolation was not valid.
    ///
    /// Unreachable when decoding a frame, since every bit of `data[2]` has
    /// a defined meaning; retained because [`ViolationType::try_from`] can
    /// still be called directly with a combined value.
    InvalidViolationType,
    /// One half of `data[4]` held an unrecognised transceiver code.
    InvalidTransceiverError,
}

impl error::Error for CanErrorDecodingFailure {}

impl fmt::Display for CanErrorDecodingFailure {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use CanErrorDecodingFailure::*;
        let msg = match *self {
            InvalidControllerProblem => "not a valid controller problem",
            InvalidViolationType => "not a valid violation type",
            InvalidTransceiverError => "not a valid transceiver error",
        };
        write!(f, "{}", msg)
    }
}

// ===== ConstructionError =====

#[derive(Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq)]
/// Error that occurs when creating CAN packets
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ConstructionError {
    /// Trying to create a specific frame type from an incompatible type
    WrongFrameType,
    /// CAN ID was outside the range of valid IDs
    IDTooLarge,
    /// Larger payload reported than can be held in the frame.
    TooMuchData,
}

impl error::Error for ConstructionError {}

impl fmt::Display for ConstructionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ConstructionError::*;
        let msg = match *self {
            WrongFrameType => "Incompatible frame type",
            IDTooLarge => "CAN ID too large",
            TooMuchData => "Payload is too large",
        };
        write!(f, "{}", msg)
    }
}

/////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CanErrorFrame, Error};
    use embedded_can::{Error as _, ErrorKind};
    use std::io;

    /// Builds an error frame from raw class bits and data bytes.
    fn frame(bits: u32, data: [u8; 8]) -> CanErrorFrame {
        CanErrorFrame::new_error(bits, &data).unwrap()
    }

    /// Decodes raw class bits + data into a vector of errors.
    fn decode(bits: u32, data: [u8; 8]) -> Vec<CanError> {
        CanErrors::from(frame(bits, data)).into_iter().collect()
    }

    #[test]
    fn test_errors() {
        const KIND: io::ErrorKind = io::ErrorKind::TimedOut;

        // From an IO error.
        let err = Error::from(io::Error::from(KIND));
        if let Error::Io(ioerr) = err {
            assert_eq!(ioerr.kind(), KIND);
        } else {
            panic!("Wrong error conversion");
        }

        // Straight from an ErrorKind
        let err = Error::from(KIND);
        if let Error::Io(ioerr) = err {
            assert_eq!(ioerr.kind(), KIND);
        } else {
            panic!("Wrong error conversion");
        }
    }

    // ----- the non-empty invariant -----

    #[test]
    fn non_empty_invariant() {
        let errs = CanErrors::from_single(CanError::BusOff);
        assert_eq!(errs.len(), 1);
        assert!(errs.is_single());
        assert!(!errs.is_empty());
        assert_eq!(*errs.first(), CanError::BusOff);
        assert_eq!(*errs.last(), CanError::BusOff);

        // A frame with no class bits at all still yields one error.
        let errs = CanErrors::from(frame(0, [0; 8]));
        assert_eq!(errs.len(), 1);
        assert_eq!(*errs.first(), CanError::Unknown(0));
    }

    #[test]
    fn single_error_does_not_allocate() {
        let errs = CanErrors::from_single(CanError::BusOff);
        assert_eq!(errs.rest.capacity(), 0);
    }

    // ----- level 1: multiple classes per frame -----

    #[test]
    fn multi_class_crtl_and_cnt() {
        // The universal controller-state-change frame. m_can and peak_canfd
        // write `cf->can_id |= CAN_ERR_CRTL | CAN_ERR_CNT` literally.
        let mut data = [0u8; 8];
        data[1] = ControllerProblem::ReceiveErrorPassive as u8;
        data[6] = 130;
        data[7] = 42;
        assert_eq!(
            decode(CAN_ERR_CRTL | CAN_ERR_CNT, data),
            vec![
                CanError::ControllerProblem(ControllerProblem::ReceiveErrorPassive),
                CanError::Counters { tx: 130, rx: 42 },
            ]
        );
    }

    #[test]
    fn multi_class_prot_and_buserror() {
        let mut data = [0u8; 8];
        data[2] = ViolationType::BitStuffingError as u8;
        data[3] = 0x08; // CRC sequence
        assert_eq!(
            decode(CAN_ERR_PROT | CAN_ERR_BUSERROR, data),
            vec![
                CanError::ProtocolViolation {
                    vtype: ViolationType::BitStuffingError,
                    location: Location::CrcSequence,
                },
                CanError::BusError,
            ]
        );
    }

    #[test]
    fn unknown_class_bits_trail() {
        // 0x400 is above every class bit we know.
        let errs = decode(CAN_ERR_BUSOFF | 0x400, [0; 8]);
        assert_eq!(errs, vec![CanError::BusOff, CanError::Unknown(0x400)]);
    }

    #[test]
    fn class_bit_ordering_is_ascending() {
        let mut data = [0u8; 8];
        data[0] = 7;
        data[1] = ControllerProblem::ReceiveBufferOverflow as u8;
        data[2] = ViolationType::SingleBitError as u8;
        data[4] = TransceiverError::CanHighNoWire as u8;
        data[6] = 1;
        data[7] = 2;
        let errs = decode(
            CAN_ERR_TX_TIMEOUT
                | CAN_ERR_LOSTARB
                | CAN_ERR_CRTL
                | CAN_ERR_PROT
                | CAN_ERR_TRX
                | CAN_ERR_ACK
                | CAN_ERR_BUSOFF
                | CAN_ERR_BUSERROR
                | CAN_ERR_RESTARTED
                | CAN_ERR_CNT,
            data,
        );
        assert_eq!(
            errs,
            vec![
                CanError::TransmitTimeout,
                CanError::LostArbitration(7),
                CanError::ControllerProblem(ControllerProblem::ReceiveBufferOverflow),
                CanError::ProtocolViolation {
                    vtype: ViolationType::SingleBitError,
                    location: Location::Unspecified,
                },
                CanError::TransceiverError(TransceiverError::CanHighNoWire),
                CanError::NoAck,
                CanError::BusOff,
                CanError::BusError,
                CanError::Restarted,
                CanError::Counters { tx: 1, rx: 2 },
            ]
        );
    }

    // ----- level 2: multiple bits within a class byte -----

    #[test]
    fn ctrl_multi_bit_symmetric_warning() {
        // What can_change_state() emits when tx_state == rx_state ==
        // CAN_STATE_ERROR_WARNING. This is the single most common real
        // multi-bit frame; it used to decode to a DecodingFailure.
        let mut data = [0u8; 8];
        data[1] = 0x0C;
        assert_eq!(
            decode(CAN_ERR_CRTL, data),
            vec![
                CanError::ControllerProblem(ControllerProblem::ReceiveErrorWarning),
                CanError::ControllerProblem(ControllerProblem::TransmitErrorWarning),
            ]
        );
    }

    #[test]
    fn ctrl_multi_bit_symmetric_passive() {
        // can_change_state() with both states ERROR_PASSIVE; also m_can's
        // CAN_STATE_ERROR_PASSIVE arm.
        let mut data = [0u8; 8];
        data[1] = 0x30;
        assert_eq!(
            decode(CAN_ERR_CRTL, data),
            vec![
                CanError::ControllerProblem(ControllerProblem::ReceiveErrorPassive),
                CanError::ControllerProblem(ControllerProblem::TransmitErrorPassive),
            ]
        );
    }

    #[test]
    fn ctrl_three_bits_sja1000_overrun_plus_warning() {
        // sja1000 sets data[1] = RX_OVERFLOW on a data overrun, then
        // can_change_state() ORs the warning bits in.
        let mut data = [0u8; 8];
        data[1] = 0x0D;
        assert_eq!(
            decode(CAN_ERR_CRTL, data),
            vec![
                CanError::ControllerProblem(ControllerProblem::ReceiveBufferOverflow),
                CanError::ControllerProblem(ControllerProblem::ReceiveErrorWarning),
                CanError::ControllerProblem(ControllerProblem::TransmitErrorWarning),
            ]
        );
    }

    #[test]
    fn ctrl_zero_is_unspecified_not_a_failure() {
        // CAN_ERR_CRTL_UNSPEC. Regression guard: a bit walk alone can never
        // produce this, and a naive implementation reports a decoding
        // failure for a perfectly valid frame.
        assert_eq!(
            decode(CAN_ERR_CRTL, [0; 8]),
            vec![CanError::ControllerProblem(ControllerProblem::Unspecified)]
        );
    }

    #[test]
    fn ctrl_unclaimed_bit_reports_failure_after_known_bits() {
        // Bit 7 of data[1] is not claimed by any known problem.
        let mut data = [0u8; 8];
        data[1] = 0x81;
        assert_eq!(
            decode(CAN_ERR_CRTL, data),
            vec![
                CanError::ControllerProblem(ControllerProblem::ReceiveBufferOverflow),
                CanError::DecodingFailure(CanErrorDecodingFailure::InvalidControllerProblem),
            ]
        );
    }

    #[test]
    fn prot_multi_bit_shares_one_location() {
        // mcp251xfd_handle_ivmif() accumulates STUFF|FORM|TX|BIT1|BIT0.
        let mut data = [0u8; 8];
        data[2] = 0x9E;
        data[3] = 0x08;
        let errs = decode(CAN_ERR_PROT, data);
        assert_eq!(errs.len(), 5);
        assert!(errs.iter().all(|e| matches!(
            e,
            CanError::ProtocolViolation {
                location: Location::CrcSequence,
                ..
            }
        )));
        let types: Vec<_> = errs
            .iter()
            .map(|e| match e {
                CanError::ProtocolViolation { vtype, .. } => *vtype,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            types,
            vec![
                ViolationType::FrameFormatError,
                ViolationType::BitStuffingError,
                ViolationType::UnableToSendDominantBit,
                ViolationType::UnableToSendRecessiveBit,
                ViolationType::TransmissionError,
            ]
        );
    }

    #[test]
    fn prot_zero_is_unspecified_not_a_failure() {
        // es58x sets CAN_ERR_PROT whenever data[2] OR data[3] is non-zero,
        // so a location-only violation with data[2] == 0 is reachable.
        let mut data = [0u8; 8];
        data[3] = 0x03;
        assert_eq!(
            decode(CAN_ERR_PROT, data),
            vec![CanError::ProtocolViolation {
                vtype: ViolationType::Unspecified,
                location: Location::StartOfFrame,
            }]
        );
    }

    // ----- data[4]: two nibbles -----

    #[test]
    fn trx_both_lines_es58x() {
        // es58x ORs CANH and CANL codes for a single-wire fault:
        //   cf->data[4] |= CAN_ERR_TRX_CANH_NO_WIRE;
        //   cf->data[4] |= CAN_ERR_TRX_CANL_NO_WIRE;
        // A scalar decode of data[4] rejects 0x44 outright.
        let mut data = [0u8; 8];
        data[4] = 0x44;
        assert_eq!(
            decode(CAN_ERR_TRX, data),
            vec![
                CanError::TransceiverError(TransceiverError::CanHighNoWire),
                CanError::TransceiverError(TransceiverError::CanLowNoWire),
            ]
        );
    }

    #[test]
    fn trx_single_line_each_half() {
        let mut data = [0u8; 8];
        data[4] = 0x05;
        assert_eq!(
            decode(CAN_ERR_TRX, data),
            vec![CanError::TransceiverError(
                TransceiverError::CanHighShortToBat
            )]
        );

        data[4] = 0x80;
        assert_eq!(
            decode(CAN_ERR_TRX, data),
            vec![CanError::TransceiverError(
                TransceiverError::CanLowShortToCanHigh
            )]
        );
    }

    #[test]
    fn trx_zero_is_unspecified() {
        assert_eq!(
            decode(CAN_ERR_TRX, [0; 8]),
            vec![CanError::TransceiverError(TransceiverError::Unspecified)]
        );
    }

    #[test]
    fn trx_invalid_half_reports_failure() {
        let mut data = [0u8; 8];
        data[4] = 0x03; // no CANH code 0x03
        assert_eq!(
            decode(CAN_ERR_TRX, data),
            vec![CanError::DecodingFailure(
                CanErrorDecodingFailure::InvalidTransceiverError
            )]
        );
    }

    // ----- data[3]: total over the whole byte -----

    #[test]
    fn location_decoding_never_fails() {
        // sja1000 writes the raw 5-bit ECC segment, so all of 0x00..=0x1F
        // is reachable; the rest must not blow up either.
        for v in 0u8..=0xFF {
            let loc = Location::from_raw(v);
            assert_eq!(loc.as_raw(), v, "round-trip failed for {:#04x}", v);
        }
    }

    #[test]
    fn location_named_beyond_error_h() {
        // Present in can-utils and emitted by sja1000, absent from
        // linux/can/error.h.
        assert_eq!(Location::from_raw(0x11), Location::ActiveErrorFlag);
        assert_eq!(Location::from_raw(0x13), Location::TolerateDominantBits);
        assert_eq!(Location::from_raw(0x16), Location::PassiveErrorFlag);
        assert_eq!(Location::from_raw(0x17), Location::ErrorDelimiter);
        assert_eq!(Location::from_raw(0x1C), Location::OverloadFlag);
    }

    #[test]
    fn location_unnamed_in_range_is_reserved() {
        for v in [0x01u8, 0x10, 0x14, 0x15, 0x1D, 0x1E, 0x1F] {
            assert_eq!(Location::from_raw(v), Location::Reserved(v));
        }
    }

    // ----- error kinds -----

    #[test]
    fn kind_prefers_specific_over_other() {
        // A controller warning maps only to Other, so the missing ACK must
        // win. Scanning by class in declaration order gets this wrong.
        let mut data = [0u8; 8];
        data[1] = 0x0C;
        let errs = CanErrors::from(frame(CAN_ERR_CRTL | CAN_ERR_ACK, data));
        assert_eq!(errs.kind(), ErrorKind::Acknowledge);
        assert!(errs.contains_kind(ErrorKind::Acknowledge));
        assert!(errs.contains_kind(ErrorKind::Other));
    }

    #[test]
    fn kind_maps_violation_types() {
        let check = |vtype: ViolationType, expect: ErrorKind| {
            let err = CanError::ProtocolViolation {
                vtype,
                location: Location::Unspecified,
            };
            assert_eq!(err.kind(), expect, "for {:?}", vtype);
        };
        check(ViolationType::SingleBitError, ErrorKind::Bit);
        check(ViolationType::UnableToSendDominantBit, ErrorKind::Bit);
        check(ViolationType::UnableToSendRecessiveBit, ErrorKind::Bit);
        check(ViolationType::FrameFormatError, ErrorKind::Form);
        check(ViolationType::BitStuffingError, ErrorKind::Stuff);
        check(ViolationType::BusOverload, ErrorKind::Other);
    }

    #[test]
    fn kind_overrun_from_buffer_overflow() {
        let mut data = [0u8; 8];
        data[1] = ControllerProblem::TransmitBufferOverflow as u8;
        let errs = CanErrors::from(frame(CAN_ERR_CRTL, data));
        assert_eq!(errs.kind(), ErrorKind::Overrun);
    }

    #[test]
    fn kind_all_other_falls_back() {
        let errs = CanErrors::from(frame(CAN_ERR_BUSOFF | CAN_ERR_RESTARTED, [0; 8]));
        assert_eq!(errs.kind(), ErrorKind::Other);
    }

    #[test]
    fn top_level_error_delegates_kind() {
        let mut data = [0u8; 8];
        data[1] = 0x0C;
        let err = Error::from(frame(CAN_ERR_CRTL | CAN_ERR_ACK, data));
        assert_eq!(err.kind(), ErrorKind::Acknowledge);
    }

    // ----- Display -----

    #[test]
    fn display_single_is_bare() {
        let errs = CanErrors::from_single(CanError::BusOff);
        assert_eq!(errs.to_string(), "bus off");
    }

    #[test]
    fn display_multi_is_semicolon_joined() {
        let mut data = [0u8; 8];
        data[1] = 0x0C;
        data[6] = 96;
        data[7] = 0;
        let errs = CanErrors::from(frame(CAN_ERR_CRTL | CAN_ERR_CNT, data));
        assert_eq!(
            errs.to_string(),
            "controller problem: ERROR WARNING (receive); \
             controller problem: ERROR WARNING (transmit); \
             error counters: tx=96, rx=0"
        );
    }

    #[test]
    fn display_groups_violations_sharing_a_location() {
        // Without grouping this would repeat "at CRC sequence" five times.
        let mut data = [0u8; 8];
        data[2] = 0x9E;
        data[3] = 0x08;
        let errs = CanErrors::from(frame(CAN_ERR_PROT | CAN_ERR_BUSERROR, data));
        assert_eq!(
            errs.to_string(),
            "protocol violation at CRC sequence: frame format error, \
             bit stuffing error, unable to send dominant bit, \
             unable to send recessive bit, error on transmission; bus error"
        );
    }

    #[test]
    fn display_unknown_in_hex() {
        let errs = CanErrors::from_single(CanError::Unknown(0x400));
        assert_eq!(errs.to_string(), "unknown error (0x400)");
    }

    // ----- iteration / conversion plumbing -----

    #[test]
    fn iteration_by_value_and_by_ref() {
        let errs = CanErrors::new(CanError::BusOff, [CanError::NoAck, CanError::Restarted]);
        assert_eq!(errs.len(), 3);
        assert!(!errs.is_single());
        assert_eq!(*errs.last(), CanError::Restarted);

        let by_ref: Vec<_> = (&errs).into_iter().copied().collect();
        let by_val: Vec<_> = errs.clone().into_iter().collect();
        assert_eq!(by_ref, by_val);
        assert_eq!(by_ref.len(), 3);

        let via_iter: Vec<_> = errs.iter().copied().collect();
        assert_eq!(via_iter, by_val);
    }

    #[test]
    fn single_can_error_promotes_to_collection() {
        let errs: CanErrors = CanError::BusOff.into();
        assert!(errs.is_single());

        // ... and through the top-level Error.
        let err: Error = CanError::BusOff.into();
        match err {
            Error::Can(errs) => assert_eq!(*errs.first(), CanError::BusOff),
            _ => panic!("expected a CAN error"),
        }
    }

    #[test]
    fn from_iter_checked_rejects_empty() {
        assert!(CanErrors::from_iter_checked(std::iter::empty()).is_none());
        assert!(CanErrors::from_iter_checked([CanError::BusOff]).is_some());
    }
}
