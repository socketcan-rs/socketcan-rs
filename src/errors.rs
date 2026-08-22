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
//! # One error, several causes
//!
//! A single error frame describes **one** error event, but that event can
//! have several distinct causes, at two levels:
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
//! Decoding therefore yields a single [`CanError`] holding one [`ErrorCause`]
//! per class bit. The two bitfield facets ([`ErrorCause::Controller`],
//! [`ErrorCause::Protocol`]) and the two-nibble [`ErrorCause::Transceiver`]
//! each carry a *set* of conditions in one cause, rather than exploding into
//! sibling entries. A [`CanError`] is non-empty by construction: there is
//! always a [`first`](CanError::first) cause, in ascending class-bit order.
//!
//! All of this error information is not well documented, but can be
//! extracted from the Linux kernel header file:
//! [linux/can/error.h](https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/can/error.h)
//!

use crate::{CanErrorFrame, EmbeddedFrame, Frame};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};
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
/// state. Compare against the counters from [`ErrorCause::Counters`].
pub use libc::CAN_ERROR_WARNING_THRESHOLD;

/// The error counter value at which a controller enters the "error passive"
/// state. Compare against the counters from [`ErrorCause::Counters`].
pub use libc::CAN_ERROR_PASSIVE_THRESHOLD;

/// The error counter value at which a controller goes bus-off.
/// Compare against the counters from [`ErrorCause::Counters`].
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
    /// A CAN error decoded from an error frame.
    #[error(transparent)]
    Can(#[from] CanError),
    /// An I/O Error
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl embedded_can::Error for Error {
    fn kind(&self) -> embedded_can::ErrorKind {
        match self {
            Error::Can(err) => err.kind(),
            _ => embedded_can::ErrorKind::Other,
        }
    }
}

impl From<ErrorCause> for Error {
    /// Wraps a single cause, promoting it to a one-cause [`CanError`].
    fn from(cause: ErrorCause) -> Self {
        Error::Can(CanError::new(cause))
    }
}

impl From<CanErrorFrame> for Error {
    fn from(frame: CanErrorFrame) -> Self {
        Error::Can(CanError::from(frame))
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

// ===== CanError =====

/// Inline capacity for the cause list. Real frames report 1–3 causes; the
/// theoretical maximum (~13) is synthetic, so anything larger spills to the
/// heap. The common single-cause case never allocates.
const INLINE_CAUSES: usize = 4;

/// The backing storage for a [`CanError`]'s causes.
type Causes = SmallVec<[ErrorCause; INLINE_CAUSES]>;

/// The error decoded from a single CAN error frame.
///
/// A SocketCAN error frame reports *one* error event, described by one or
/// more [`ErrorCause`]s — most commonly a controller state change together
/// with the current TX/RX error counter values, or a bus error annotated
/// with several protocol violation types and a location. See the [module
/// documentation](self) for the two levels at which several causes arise.
///
/// `CanError` is **non-empty by construction**: there is always a
/// [`first()`](Self::first) cause. A frame with no recognisable error bits
/// decodes to a single [`ErrorCause::Unknown`] rather than an empty error.
///
/// # Ordering
///
/// Causes appear in a stable, documented order: error classes in ascending
/// numeric order of their CAN ID bit, i.e. TX timeout, lost arbitration,
/// controller problem, protocol violation, transceiver status, no-ACK, bus
/// off, bus error, restarted, counters, followed by any unrecognised class
/// bits as a trailing [`ErrorCause::Unknown`].
///
/// # Implementation
///
/// The causes are held inline for the common small case, so decoding a
/// single- or few-cause frame does not allocate.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(into = "Vec<ErrorCause>", try_from = "Vec<ErrorCause>")
)]
pub struct CanError {
    causes: Causes,
}

impl CanError {
    /// Creates an error holding exactly one cause.
    ///
    /// This does not allocate.
    pub fn new(cause: ErrorCause) -> Self {
        Self {
            causes: smallvec![cause],
        }
    }

    /// Creates an error from a first cause plus any number of additional ones.
    pub fn from_multiple(first: ErrorCause, rest: impl IntoIterator<Item = ErrorCause>) -> Self {
        let mut causes = Causes::new();
        causes.push(first);
        causes.extend(rest);
        Self { causes }
    }

    /// Creates an error from an iterator of causes, returning `None` if it is
    /// empty.
    ///
    /// Prefer [`new()`](Self::new) or [`from_multiple()`](Self::from_multiple)
    /// when the non-emptiness is already known statically.
    pub fn from_iter_checked(causes: impl IntoIterator<Item = ErrorCause>) -> Option<Self> {
        let causes: Causes = causes.into_iter().collect();
        (!causes.is_empty()).then_some(Self { causes })
    }

    /// Gets the first cause.
    ///
    /// This is never `None`: the type is non-empty by construction. For a
    /// frame that set several class bits, this is the one belonging to the
    /// lowest-numbered class bit.
    pub fn first(&self) -> &ErrorCause {
        &self.causes[0]
    }

    /// Gets the last cause.
    pub fn last(&self) -> &ErrorCause {
        self.causes.last().unwrap()
    }

    /// The number of causes reported. Always at least one.
    pub fn len(&self) -> usize {
        self.causes.len()
    }

    /// Always `false`; the error is non-empty by construction.
    ///
    /// Provided only because clippy expects `is_empty` alongside `len`.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Determines whether this holds exactly one cause.
    ///
    /// Note that multi-cause errors are *common*, not exceptional: any
    /// controller state change reports `CAN_ERR_CRTL | CAN_ERR_CNT`, which
    /// is two causes. Do not treat the single case as the norm.
    pub fn is_single(&self) -> bool {
        self.causes.len() == 1
    }

    /// An iterator over the causes, in the order documented on the type.
    pub fn causes(&self) -> impl Iterator<Item = &ErrorCause> + '_ {
        self.causes.iter()
    }

    /// Determines whether any of the causes maps to the given
    /// [`embedded_can::ErrorKind`].
    pub fn contains_kind(&self, kind: embedded_can::ErrorKind) -> bool {
        use embedded_can::Error as _;
        self.causes().any(|c| c.kind() == kind)
    }

    // ----- typed accessors for the data-carrying causes -----

    /// The bit position after which arbitration was lost, if reported.
    ///
    /// Note that the kernel uses zero to mean *unspecified* rather than
    /// literally "bit 0".
    pub fn lost_arbitration(&self) -> Option<u8> {
        self.causes().find_map(|c| match c {
            ErrorCause::LostArbitration(bit) => Some(*bit),
            _ => None,
        })
    }

    /// The controller status flags, if the frame carried them.
    pub fn controller(&self) -> Option<ControllerProblems> {
        self.causes().find_map(|c| match c {
            ErrorCause::Controller(p) => Some(*p),
            _ => None,
        })
    }

    /// The protocol violation type(s) and location, if reported.
    pub fn protocol(&self) -> Option<(ViolationTypes, Location)> {
        self.causes().find_map(|c| match c {
            ErrorCause::Protocol { types, location } => Some((*types, *location)),
            _ => None,
        })
    }

    /// The CAN High and CAN Low line faults, if a transceiver status was
    /// reported.
    pub fn transceiver(&self) -> Option<(Option<CanHighFault>, Option<CanLowFault>)> {
        self.causes().find_map(|c| match c {
            ErrorCause::Transceiver { canh, canl } => Some((*canh, *canl)),
            _ => None,
        })
    }

    /// The TX/RX error counters, if the frame carried a `CAN_ERR_CNT` cause.
    pub fn counters(&self) -> Option<(u8, u8)> {
        self.causes().find_map(|c| match c {
            ErrorCause::Counters { tx, rx } => Some((*tx, *rx)),
            _ => None,
        })
    }
}

/// Generates a boolean predicate over the causes.
macro_rules! cause_predicate {
    ($name:ident, $doc:literal, $pat:pat) => {
        #[doc = $doc]
        pub fn $name(&self) -> bool {
            self.causes().any(|c| matches!(c, $pat))
        }
    };
}

impl CanError {
    cause_predicate!(
        is_transmit_timeout,
        "Whether a TX timeout was reported.",
        ErrorCause::TransmitTimeout
    );
    cause_predicate!(
        is_no_ack,
        "Whether the frame went unacknowledged.",
        ErrorCause::NoAck
    );
    cause_predicate!(
        is_bus_off,
        "Whether the controller reported a bus-off condition.",
        ErrorCause::BusOff
    );
    cause_predicate!(
        is_bus_error,
        "Whether a bus error was reported.",
        ErrorCause::BusError
    );
    cause_predicate!(
        is_restarted,
        "Whether the controller restarted.",
        ErrorCause::Restarted
    );
    cause_predicate!(
        has_counters,
        "Whether error counter values were reported.",
        ErrorCause::Counters { .. }
    );
}

impl From<ErrorCause> for CanError {
    fn from(cause: ErrorCause) -> Self {
        Self::new(cause)
    }
}

impl IntoIterator for CanError {
    type Item = ErrorCause;
    type IntoIter = smallvec::IntoIter<[ErrorCause; INLINE_CAUSES]>;

    fn into_iter(self) -> Self::IntoIter {
        self.causes.into_iter()
    }
}

impl<'a> IntoIterator for &'a CanError {
    type Item = &'a ErrorCause;
    type IntoIter = std::slice::Iter<'a, ErrorCause>;

    fn into_iter(self) -> Self::IntoIter {
        self.causes.iter()
    }
}

impl error::Error for CanError {}

impl fmt::Display for CanError {
    /// Renders the causes as a single line.
    ///
    /// A lone cause renders exactly as its own `Display`. Multiple causes are
    /// joined with "; ".
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut first = true;
        for cause in self.causes() {
            if !first {
                write!(f, "; ")?;
            }
            first = false;
            write!(f, "{}", cause)?;
        }
        Ok(())
    }
}

impl embedded_can::Error for CanError {
    /// Reports the most specific error kind present.
    ///
    /// Scans the causes in order and returns the first kind that is not
    /// [`ErrorKind::Other`](embedded_can::ErrorKind::Other), falling back to
    /// `Other` when every cause is unspecific. The scan is over *kinds*, not
    /// over error classes in declaration order — a frame carrying both a
    /// controller warning and a missing ACK reports `Acknowledge`, since the
    /// warning maps only to `Other`.
    fn kind(&self) -> embedded_can::ErrorKind {
        use embedded_can::ErrorKind;
        self.causes()
            .map(|c| c.kind())
            .find(|k| *k != ErrorKind::Other)
            .unwrap_or(ErrorKind::Other)
    }
}

impl From<CanErrorFrame> for CanError {
    /// Decodes every cause described by an error frame.
    ///
    /// Walks the class bits of the CAN ID in ascending order, producing one
    /// [`ErrorCause`] per class bit. The bitfield facets carry a set, so a
    /// class describing several simultaneous conditions is a single cause.
    fn from(frame: CanErrorFrame) -> Self {
        // Note that the CanErrorFrame is guaranteed to have the full 8-byte
        // data payload.
        let bits = frame.error_bits();
        let data = frame.data();
        let mut causes = Causes::new();

        if bits & CAN_ERR_TX_TIMEOUT != 0 {
            causes.push(ErrorCause::TransmitTimeout);
        }
        if bits & CAN_ERR_LOSTARB != 0 {
            causes.push(ErrorCause::LostArbitration(data[0]));
        }
        if bits & CAN_ERR_CRTL != 0 {
            push_controller(&mut causes, data[1]);
        }
        if bits & CAN_ERR_PROT != 0 {
            // Every bit of data[2] is defined, so this cannot drop bits.
            causes.push(ErrorCause::Protocol {
                types: ViolationTypes::from_bits_truncate(data[2]),
                location: Location::from_raw(data[3]),
            });
        }
        if bits & CAN_ERR_TRX != 0 {
            push_transceiver(&mut causes, data[4]);
        }
        if bits & CAN_ERR_ACK != 0 {
            causes.push(ErrorCause::NoAck);
        }
        if bits & CAN_ERR_BUSOFF != 0 {
            causes.push(ErrorCause::BusOff);
        }
        if bits & CAN_ERR_BUSERROR != 0 {
            causes.push(ErrorCause::BusError);
        }
        if bits & CAN_ERR_RESTARTED != 0 {
            causes.push(ErrorCause::Restarted);
        }
        // Strictly gated on the flag. The kernel leaves data[6..7] undefined
        // when CAN_ERR_CNT is clear; can-utils prints them anyway, but that
        // is a display convenience, not a decoding rule.
        if bits & CAN_ERR_CNT != 0 {
            causes.push(ErrorCause::Counters {
                tx: data[6],
                rx: data[7],
            });
        }

        // Any class bits we do not recognise are reported as a single
        // trailing cause carrying just those bits.
        let unknown = bits & !KNOWN_ERR_CLASSES;
        if unknown != 0 {
            causes.push(ErrorCause::Unknown(unknown));
        }

        // A frame with no class bits at all is malformed; report it rather
        // than violating the non-empty invariant.
        if causes.is_empty() {
            causes.push(ErrorCause::Unknown(0));
        }
        Self { causes }
    }
}

/// Decodes the controller-problem bitfield in `data[1]` into one
/// [`ErrorCause::Controller`]. An empty set is `CAN_ERR_CRTL_UNSPEC`.
///
/// Any bits with no known meaning produce a trailing
/// [`ErrorCause::DecodingFailure`].
fn push_controller(causes: &mut Causes, byte: u8) {
    causes.push(ErrorCause::Controller(
        ControllerProblems::from_bits_truncate(byte),
    ));
    if byte & !ControllerProblems::all().bits() != 0 {
        causes.push(ErrorCause::DecodingFailure(
            CanErrorDecodingFailure::InvalidControllerProblem,
        ));
    }
}

/// Decodes the two nibbles of `data[4]` into one [`ErrorCause::Transceiver`].
///
/// The low nibble describes the CAN High line and the high nibble the CAN Low
/// line, so a fault on both lines is a single byte with both halves set (the
/// kernel's `etas_es58x` driver emits `0x44` for a lost connection on either
/// line). A zero half is absent (`None`). An unrecognised half yields a
/// trailing [`ErrorCause::DecodingFailure`].
fn push_transceiver(causes: &mut Causes, byte: u8) {
    let mut invalid = false;
    let canh = match byte & 0x0F {
        0 => None,
        h => CanHighFault::try_from(h).map(Some).unwrap_or_else(|_| {
            invalid = true;
            None
        }),
    };
    let canl = match byte & 0xF0 {
        0 => None,
        l => CanLowFault::try_from(l).map(Some).unwrap_or_else(|_| {
            invalid = true;
            None
        }),
    };
    causes.push(ErrorCause::Transceiver { canh, canl });
    if invalid {
        causes.push(ErrorCause::DecodingFailure(
            CanErrorDecodingFailure::InvalidTransceiverError,
        ));
    }
}

/////////////////////////////////////////////////////////////////////////////
// serde support for the composite error and the CAN error

/// Serialized form of [`enum@Error`].
///
/// The `Can` half round-trips exactly. The `Io` half cannot: `io::Error`
/// implements neither serde trait and may carry an OS errno or a boxed source,
/// so it is reduced to its kind and message. See [`ErrorRepr::Io`].
#[cfg(feature = "serde")]
#[derive(Debug, Serialize, Deserialize)]
pub enum ErrorRepr {
    /// A CAN error
    Can(CanError),
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
/// This clones the [`CanError`] to build the repr. Serialization is not a hot
/// path, so the allocation is not worth avoiding with a parallel borrowing
/// repr that would have to be kept in sync by hand.
#[cfg(feature = "serde")]
impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> std::result::Result<S::Ok, S::Error> {
        let repr = match self {
            Error::Can(err) => ErrorRepr::Can(err.clone()),
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
            ErrorRepr::Can(err) => Self::Can(err),
            ErrorRepr::Io { kind, message } => {
                Self::Io(io::Error::new(io_kind_from_name(&kind), message))
            }
        }
    }
}

#[cfg(feature = "serde")]
impl From<CanError> for Vec<ErrorCause> {
    fn from(err: CanError) -> Self {
        err.into_iter().collect()
    }
}

#[cfg(feature = "serde")]
impl TryFrom<Vec<ErrorCause>> for CanError {
    type Error = EmptyCanError;

    /// Rebuilds the error, rejecting an empty sequence.
    ///
    /// This is what keeps the non-empty invariant intact across
    /// deserialization; without it, serde would be a way to construct an
    /// invalid `CanError` from outside the crate.
    fn try_from(causes: Vec<ErrorCause>) -> std::result::Result<Self, Self::Error> {
        Self::from_iter_checked(causes).ok_or(EmptyCanError)
    }
}

/// Error returned when deserializing a [`CanError`] from an empty sequence.
///
/// [`CanError`] is non-empty by construction, so an empty input is invalid
/// rather than merely unusual.
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyCanError;

#[cfg(feature = "serde")]
impl fmt::Display for EmptyCanError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a CanError must hold at least one cause")
    }
}

#[cfg(feature = "serde")]
impl error::Error for EmptyCanError {}

// ===== ErrorCause ====

/// A single condition a CAN error frame reported.
///
/// One `ErrorCause` corresponds to one class bit in the frame's CAN ID. The
/// two bitfield facets ([`Controller`](Self::Controller),
/// [`Protocol`](Self::Protocol)) and the two-nibble
/// [`Transceiver`](Self::Transceiver) byte each carry a *set* of conditions
/// in a single cause, rather than exploding into sibling entries.
///
/// A frame as a whole decodes to a [`CanError`], which holds these; see the
/// [module documentation](self).
///
/// This is `#[non_exhaustive]`: new class bits may be added in future kernels,
/// so downstream `match`es should include a wildcard arm.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ErrorCause {
    /// TX timeout (by netdevice driver). `CAN_ERR_TX_TIMEOUT`.
    TransmitTimeout,
    /// Arbitration was lost.
    ///
    /// Contains the bit number after which arbitration was lost. Note that
    /// the kernel uses zero (`CAN_ERR_LOSTARB_UNSPEC`) to mean
    /// *unspecified* rather than literally "bit 0". `CAN_ERR_LOSTARB`.
    LostArbitration(u8),
    /// Controller status flags, from `data[1]`. `CAN_ERR_CRTL`.
    ///
    /// An empty set is `CAN_ERR_CRTL_UNSPEC`.
    Controller(ControllerProblems),
    /// Protocol violation(s) at one location, from `data[2..=3]`.
    /// `CAN_ERR_PROT`.
    Protocol {
        /// The violation type(s); an empty set means "unspecified".
        types: ViolationTypes,
        /// The location (field or bit) of the violation.
        location: Location,
    },
    /// Transceiver line faults, from the two nibbles of `data[4]`.
    /// `CAN_ERR_TRX`.
    Transceiver {
        /// The CAN High line fault (low nibble), if any.
        canh: Option<CanHighFault>,
        /// The CAN Low line fault (high nibble), if any.
        canl: Option<CanLowFault>,
    },
    /// No ACK received for the transmitted frame. `CAN_ERR_ACK`.
    NoAck,
    /// Bus off (due to too many detected errors). `CAN_ERR_BUSOFF`.
    BusOff,
    /// Bus error (due to too many detected errors). `CAN_ERR_BUSERROR`.
    BusError,
    /// The controller has been restarted. `CAN_ERR_RESTARTED`.
    Restarted,
    /// The controller's TX and RX error counter values, from a
    /// `CAN_ERR_CNT` frame (`data[6..=7]`).
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
    /// A data byte held a bit pattern this crate could not decode.
    DecodingFailure(CanErrorDecodingFailure),
    /// Unknown, possibly invalid, error class bits.
    Unknown(u32),
}

impl error::Error for ErrorCause {}

impl fmt::Display for ErrorCause {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ErrorCause::*;
        match *self {
            TransmitTimeout => write!(f, "transmission timeout"),
            LostArbitration(n) => write!(f, "arbitration lost after {} bits", n),
            Controller(p) => write!(f, "controller problem: {}", p),
            Protocol { types, location } => {
                write!(f, "protocol violation at {}: {}", location, types)
            }
            Transceiver { canh, canl } => {
                write!(f, "transceiver error: ")?;
                match (canh, canl) {
                    (Some(h), Some(l)) => write!(f, "CAN High, {}; CAN Low, {}", h, l),
                    (Some(h), None) => write!(f, "CAN High, {}", h),
                    (None, Some(l)) => write!(f, "CAN Low, {}", l),
                    (None, None) => write!(f, "unspecified"),
                }
            }
            NoAck => write!(f, "no ack"),
            BusOff => write!(f, "bus off"),
            BusError => write!(f, "bus error"),
            Restarted => write!(f, "restarted"),
            Counters { tx, rx } => write!(f, "error counters: tx={}, rx={}", tx, rx),
            DecodingFailure(err) => write!(f, "decoding failure: {}", err),
            Unknown(bits) => write!(f, "unknown error ({:#x})", bits),
        }
    }
}

impl embedded_can::Error for ErrorCause {
    fn kind(&self) -> embedded_can::ErrorKind {
        use embedded_can::ErrorKind;
        match *self {
            ErrorCause::Controller(p) => {
                if p.intersects(ControllerProblems::RX_OVERFLOW | ControllerProblems::TX_OVERFLOW) {
                    ErrorKind::Overrun
                } else {
                    ErrorKind::Other
                }
            }
            ErrorCause::Protocol { types, .. } => {
                if types
                    .intersects(ViolationTypes::BIT | ViolationTypes::BIT0 | ViolationTypes::BIT1)
                {
                    ErrorKind::Bit
                } else if types.contains(ViolationTypes::FORM) {
                    ErrorKind::Form
                } else if types.contains(ViolationTypes::STUFF) {
                    ErrorKind::Stuff
                } else {
                    ErrorKind::Other
                }
            }
            ErrorCause::NoAck => ErrorKind::Acknowledge,
            _ => ErrorKind::Other,
        }
    }
}

// ===== ControllerProblems =====

bitflags::bitflags! {
    /// Error status flags of the CAN controller.
    ///
    /// Decoded from `data[1]` of an error frame, which is a **bitfield** —
    /// several of these can be set at once. The kernel's shared
    /// `can_change_state()` helper ORs the TX and RX state codes together
    /// whenever the two states match, so pairs such as `RX_WARNING` plus
    /// `TX_WARNING` are the normal encoding rather than an anomaly. An empty
    /// set is `CAN_ERR_CRTL_UNSPEC` ("unspecified").
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct ControllerProblems: u8 {
        /// RX buffer overflow
        const RX_OVERFLOW = libc::CAN_ERR_CRTL_RX_OVERFLOW as u8;
        /// TX buffer overflow
        const TX_OVERFLOW = libc::CAN_ERR_CRTL_TX_OVERFLOW as u8;
        /// reached warning level for RX errors
        const RX_WARNING = libc::CAN_ERR_CRTL_RX_WARNING as u8;
        /// reached warning level for TX errors
        const TX_WARNING = libc::CAN_ERR_CRTL_TX_WARNING as u8;
        /// reached error-passive status, RX
        const RX_PASSIVE = libc::CAN_ERR_CRTL_RX_PASSIVE as u8;
        /// reached error-passive status, TX
        const TX_PASSIVE = libc::CAN_ERR_CRTL_TX_PASSIVE as u8;
        /// recovered to error-active state
        const ACTIVE = libc::CAN_ERR_CRTL_ACTIVE as u8;
    }
}

impl fmt::Display for ControllerProblems {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("unspecified controller problem");
        }
        const NAMED: [(ControllerProblems, &str); 7] = [
            (ControllerProblems::RX_OVERFLOW, "receive buffer overflow"),
            (ControllerProblems::TX_OVERFLOW, "transmit buffer overflow"),
            (ControllerProblems::RX_WARNING, "rx warning"),
            (ControllerProblems::TX_WARNING, "tx warning"),
            (ControllerProblems::RX_PASSIVE, "rx passive"),
            (ControllerProblems::TX_PASSIVE, "tx passive"),
            (ControllerProblems::ACTIVE, "back to error active"),
        ];
        let mut first = true;
        for (flag, msg) in NAMED {
            if self.contains(flag) {
                if !first {
                    f.write_str(", ")?;
                }
                first = false;
                f.write_str(msg)?;
            }
        }
        Ok(())
    }
}

// ===== ViolationTypes =====

bitflags::bitflags! {
    /// The type(s) of a protocol violation error.
    ///
    /// Decoded from `data[2]` of an error frame, which is a **bitfield** —
    /// several of these can be set at once. Every bit is defined, so decoding
    /// this byte can never fail. An empty set means "unspecified". Note that
    /// [`TX`](Self::TX) (`CAN_ERR_PROT_TX`) is really a direction annotation
    /// meaning "the error occurred while transmitting"; drivers OR it
    /// alongside a specific type rather than reporting it alone.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct ViolationTypes: u8 {
        /// single bit error
        const BIT = libc::CAN_ERR_PROT_BIT as u8;
        /// frame format error
        const FORM = libc::CAN_ERR_PROT_FORM as u8;
        /// bit stuffing error
        const STUFF = libc::CAN_ERR_PROT_STUFF as u8;
        /// unable to send dominant bit
        const BIT0 = libc::CAN_ERR_PROT_BIT0 as u8;
        /// unable to send recessive bit
        const BIT1 = libc::CAN_ERR_PROT_BIT1 as u8;
        /// bus overload
        const OVERLOAD = libc::CAN_ERR_PROT_OVERLOAD as u8;
        /// active error announcement
        const ACTIVE = libc::CAN_ERR_PROT_ACTIVE as u8;
        /// error occurred on transmission
        const TX = libc::CAN_ERR_PROT_TX as u8;
    }
}

impl fmt::Display for ViolationTypes {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("unspecified");
        }
        const NAMED: [(ViolationTypes, &str); 8] = [
            (ViolationTypes::BIT, "single bit error"),
            (ViolationTypes::FORM, "frame format error"),
            (ViolationTypes::STUFF, "bit stuffing error"),
            (ViolationTypes::BIT0, "unable to send dominant bit"),
            (ViolationTypes::BIT1, "unable to send recessive bit"),
            (ViolationTypes::OVERLOAD, "bus overload"),
            (ViolationTypes::ACTIVE, "active error announcement"),
            (ViolationTypes::TX, "error on transmission"),
        ];
        let mut first = true;
        for (flag, msg) in NAMED {
            if self.contains(flag) {
                if !first {
                    f.write_str(", ")?;
                }
                first = false;
                f.write_str(msg)?;
            }
        }
        Ok(())
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

// ===== Transceiver faults =====

/// A fault on the CAN High line.
///
/// Decoded from the low nibble of `data[4]` of an error frame. See
/// [`ErrorCause::Transceiver`].
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum CanHighFault {
    /// no wire
    NoWire = libc::CAN_ERR_TRX_CANH_NO_WIRE as u8,
    /// short to BAT
    ShortToBat = libc::CAN_ERR_TRX_CANH_SHORT_TO_BAT as u8,
    /// short to VCC
    ShortToVcc = libc::CAN_ERR_TRX_CANH_SHORT_TO_VCC as u8,
    /// short to GND
    ShortToGnd = libc::CAN_ERR_TRX_CANH_SHORT_TO_GND as u8,
}

impl TryFrom<u8> for CanHighFault {
    type Error = CanErrorDecodingFailure;

    /// Decodes the CAN High nibble (low nibble of `data[4]`).
    fn try_from(val: u8) -> std::result::Result<Self, Self::Error> {
        use CanHighFault::*;
        Ok(match val {
            0x04 => NoWire,
            0x05 => ShortToBat,
            0x06 => ShortToVcc,
            0x07 => ShortToGnd,
            _ => return Err(CanErrorDecodingFailure::InvalidTransceiverError),
        })
    }
}

impl fmt::Display for CanHighFault {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::NoWire => "no wire",
            Self::ShortToBat => "short to BAT",
            Self::ShortToVcc => "short to VCC",
            Self::ShortToGnd => "short to GND",
        })
    }
}

/// A fault on the CAN Low line.
///
/// Decoded from the high nibble of `data[4]` of an error frame. See
/// [`ErrorCause::Transceiver`].
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum CanLowFault {
    /// no wire
    NoWire = libc::CAN_ERR_TRX_CANL_NO_WIRE as u8,
    /// short to BAT
    ShortToBat = libc::CAN_ERR_TRX_CANL_SHORT_TO_BAT as u8,
    /// short to VCC
    ShortToVcc = libc::CAN_ERR_TRX_CANL_SHORT_TO_VCC as u8,
    /// short to GND
    ShortToGnd = libc::CAN_ERR_TRX_CANL_SHORT_TO_GND as u8,
    /// short to CAN High
    ShortToCanHigh = libc::CAN_ERR_TRX_CANL_SHORT_TO_CANH as u8,
}

impl TryFrom<u8> for CanLowFault {
    type Error = CanErrorDecodingFailure;

    /// Decodes the CAN Low nibble (high nibble of `data[4]`).
    fn try_from(val: u8) -> std::result::Result<Self, Self::Error> {
        use CanLowFault::*;
        Ok(match val {
            0x40 => NoWire,
            0x50 => ShortToBat,
            0x60 => ShortToVcc,
            0x70 => ShortToGnd,
            0x80 => ShortToCanHigh,
            _ => return Err(CanErrorDecodingFailure::InvalidTransceiverError),
        })
    }
}

impl fmt::Display for CanLowFault {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::NoWire => "no wire",
            Self::ShortToBat => "short to BAT",
            Self::ShortToVcc => "short to VCC",
            Self::ShortToGnd => "short to GND",
            Self::ShortToCanHigh => "short to CAN High",
        })
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

/// Error decoding an [`ErrorCause`] from a [`CanErrorFrame`].
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
    /// One half of `data[4]` held an unrecognised transceiver code.
    InvalidTransceiverError,
}

impl error::Error for CanErrorDecodingFailure {}

impl fmt::Display for CanErrorDecodingFailure {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use CanErrorDecodingFailure::*;
        let msg = match *self {
            InvalidControllerProblem => "not a valid controller problem",
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

    /// Decodes raw class bits + data into a vector of causes.
    fn decode(bits: u32, data: [u8; 8]) -> Vec<ErrorCause> {
        CanError::from(frame(bits, data)).into_iter().collect()
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
        let err = CanError::new(ErrorCause::BusOff);
        assert_eq!(err.len(), 1);
        assert!(err.is_single());
        assert!(!err.is_empty());
        assert_eq!(*err.first(), ErrorCause::BusOff);
        assert_eq!(*err.last(), ErrorCause::BusOff);

        // A frame with no class bits at all still yields one cause.
        let err = CanError::from(frame(0, [0; 8]));
        assert_eq!(err.len(), 1);
        assert_eq!(*err.first(), ErrorCause::Unknown(0));
    }

    #[test]
    fn single_cause_does_not_allocate() {
        let err = CanError::new(ErrorCause::BusOff);
        assert!(!err.causes.spilled());
    }

    // ----- level 1: multiple classes per frame -----

    #[test]
    fn multi_class_crtl_and_cnt() {
        // The universal controller-state-change frame. m_can and peak_canfd
        // write `cf->can_id |= CAN_ERR_CRTL | CAN_ERR_CNT` literally.
        let mut data = [0u8; 8];
        data[1] = ControllerProblems::RX_PASSIVE.bits();
        data[6] = 130;
        data[7] = 42;
        assert_eq!(
            decode(CAN_ERR_CRTL | CAN_ERR_CNT, data),
            vec![
                ErrorCause::Controller(ControllerProblems::RX_PASSIVE),
                ErrorCause::Counters { tx: 130, rx: 42 },
            ]
        );
    }

    #[test]
    fn multi_class_prot_and_buserror() {
        let mut data = [0u8; 8];
        data[2] = ViolationTypes::STUFF.bits();
        data[3] = 0x08; // CRC sequence
        assert_eq!(
            decode(CAN_ERR_PROT | CAN_ERR_BUSERROR, data),
            vec![
                ErrorCause::Protocol {
                    types: ViolationTypes::STUFF,
                    location: Location::CrcSequence,
                },
                ErrorCause::BusError,
            ]
        );
    }

    #[test]
    fn unknown_class_bits_trail() {
        // 0x400 is above every class bit we know.
        let causes = decode(CAN_ERR_BUSOFF | 0x400, [0; 8]);
        assert_eq!(causes, vec![ErrorCause::BusOff, ErrorCause::Unknown(0x400)]);
    }

    #[test]
    fn class_bit_ordering_is_ascending() {
        let mut data = [0u8; 8];
        data[0] = 7;
        data[1] = ControllerProblems::RX_OVERFLOW.bits();
        data[2] = ViolationTypes::BIT.bits();
        data[4] = 0x04; // CanHigh, no wire
        data[6] = 1;
        data[7] = 2;
        let causes = decode(
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
            causes,
            vec![
                ErrorCause::TransmitTimeout,
                ErrorCause::LostArbitration(7),
                ErrorCause::Controller(ControllerProblems::RX_OVERFLOW),
                ErrorCause::Protocol {
                    types: ViolationTypes::BIT,
                    location: Location::Unspecified,
                },
                ErrorCause::Transceiver {
                    canh: Some(CanHighFault::NoWire),
                    canl: None,
                },
                ErrorCause::NoAck,
                ErrorCause::BusOff,
                ErrorCause::BusError,
                ErrorCause::Restarted,
                ErrorCause::Counters { tx: 1, rx: 2 },
            ]
        );
    }

    // ----- level 2: multiple bits fold into one cause -----

    #[test]
    fn ctrl_multi_bit_symmetric_warning() {
        // What can_change_state() emits when tx_state == rx_state ==
        // CAN_STATE_ERROR_WARNING. Folds into a single Controller cause.
        let mut data = [0u8; 8];
        data[1] = 0x0C;
        assert_eq!(
            decode(CAN_ERR_CRTL, data),
            vec![ErrorCause::Controller(
                ControllerProblems::RX_WARNING | ControllerProblems::TX_WARNING
            )]
        );
    }

    #[test]
    fn ctrl_multi_bit_symmetric_passive() {
        // can_change_state() with both states ERROR_PASSIVE.
        let mut data = [0u8; 8];
        data[1] = 0x30;
        assert_eq!(
            decode(CAN_ERR_CRTL, data),
            vec![ErrorCause::Controller(
                ControllerProblems::RX_PASSIVE | ControllerProblems::TX_PASSIVE
            )]
        );
    }

    #[test]
    fn ctrl_three_bits_sja1000_overrun_plus_warning() {
        // sja1000 sets data[1] = RX_OVERFLOW on a data overrun, then
        // can_change_state() ORs the warning bits in. One folded cause.
        let mut data = [0u8; 8];
        data[1] = 0x0D;
        assert_eq!(
            decode(CAN_ERR_CRTL, data),
            vec![ErrorCause::Controller(
                ControllerProblems::RX_OVERFLOW
                    | ControllerProblems::RX_WARNING
                    | ControllerProblems::TX_WARNING
            )]
        );
    }

    #[test]
    fn ctrl_zero_is_unspecified_not_a_failure() {
        // CAN_ERR_CRTL_UNSPEC: an empty flag set, not a decoding failure.
        assert_eq!(
            decode(CAN_ERR_CRTL, [0; 8]),
            vec![ErrorCause::Controller(ControllerProblems::empty())]
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
                ErrorCause::Controller(ControllerProblems::RX_OVERFLOW),
                ErrorCause::DecodingFailure(CanErrorDecodingFailure::InvalidControllerProblem),
            ]
        );
    }

    #[test]
    fn prot_multi_bit_shares_one_location() {
        // mcp251xfd_handle_ivmif() accumulates STUFF|FORM|TX|BIT1|BIT0 into
        // one folded Protocol cause.
        let mut data = [0u8; 8];
        data[2] = 0x9E;
        data[3] = 0x08;
        assert_eq!(
            decode(CAN_ERR_PROT, data),
            vec![ErrorCause::Protocol {
                types: ViolationTypes::FORM
                    | ViolationTypes::STUFF
                    | ViolationTypes::BIT0
                    | ViolationTypes::BIT1
                    | ViolationTypes::TX,
                location: Location::CrcSequence,
            }]
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
            vec![ErrorCause::Protocol {
                types: ViolationTypes::empty(),
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
        let mut data = [0u8; 8];
        data[4] = 0x44;
        assert_eq!(
            decode(CAN_ERR_TRX, data),
            vec![ErrorCause::Transceiver {
                canh: Some(CanHighFault::NoWire),
                canl: Some(CanLowFault::NoWire),
            }]
        );
    }

    #[test]
    fn trx_single_line_each_half() {
        let mut data = [0u8; 8];
        data[4] = 0x05;
        assert_eq!(
            decode(CAN_ERR_TRX, data),
            vec![ErrorCause::Transceiver {
                canh: Some(CanHighFault::ShortToBat),
                canl: None,
            }]
        );

        data[4] = 0x80;
        assert_eq!(
            decode(CAN_ERR_TRX, data),
            vec![ErrorCause::Transceiver {
                canh: None,
                canl: Some(CanLowFault::ShortToCanHigh),
            }]
        );
    }

    #[test]
    fn trx_zero_is_unspecified() {
        assert_eq!(
            decode(CAN_ERR_TRX, [0; 8]),
            vec![ErrorCause::Transceiver {
                canh: None,
                canl: None,
            }]
        );
    }

    #[test]
    fn trx_invalid_half_reports_failure() {
        let mut data = [0u8; 8];
        data[4] = 0x03; // no CANH code 0x03
        assert_eq!(
            decode(CAN_ERR_TRX, data),
            vec![
                ErrorCause::Transceiver {
                    canh: None,
                    canl: None,
                },
                ErrorCause::DecodingFailure(CanErrorDecodingFailure::InvalidTransceiverError),
            ]
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
        let err = CanError::from(frame(CAN_ERR_CRTL | CAN_ERR_ACK, data));
        assert_eq!(err.kind(), ErrorKind::Acknowledge);
        assert!(err.contains_kind(ErrorKind::Acknowledge));
        assert!(err.contains_kind(ErrorKind::Other));
    }

    #[test]
    fn kind_maps_violation_types() {
        let check = |types: ViolationTypes, expect: ErrorKind| {
            let cause = ErrorCause::Protocol {
                types,
                location: Location::Unspecified,
            };
            assert_eq!(cause.kind(), expect, "for {:?}", types);
        };
        check(ViolationTypes::BIT, ErrorKind::Bit);
        check(ViolationTypes::BIT0, ErrorKind::Bit);
        check(ViolationTypes::BIT1, ErrorKind::Bit);
        check(ViolationTypes::FORM, ErrorKind::Form);
        check(ViolationTypes::STUFF, ErrorKind::Stuff);
        check(ViolationTypes::OVERLOAD, ErrorKind::Other);
    }

    #[test]
    fn kind_overrun_from_buffer_overflow() {
        let mut data = [0u8; 8];
        data[1] = ControllerProblems::TX_OVERFLOW.bits();
        let err = CanError::from(frame(CAN_ERR_CRTL, data));
        assert_eq!(err.kind(), ErrorKind::Overrun);
    }

    #[test]
    fn kind_all_other_falls_back() {
        let err = CanError::from(frame(CAN_ERR_BUSOFF | CAN_ERR_RESTARTED, [0; 8]));
        assert_eq!(err.kind(), ErrorKind::Other);
    }

    #[test]
    fn top_level_error_delegates_kind() {
        let mut data = [0u8; 8];
        data[1] = 0x0C;
        let err = Error::from(frame(CAN_ERR_CRTL | CAN_ERR_ACK, data));
        assert_eq!(err.kind(), ErrorKind::Acknowledge);
    }

    // ----- predicates and accessors -----

    #[test]
    fn predicates_and_accessors() {
        let mut data = [0u8; 8];
        data[1] = 0x0C;
        data[6] = 96;
        data[7] = 0;
        let err = CanError::from(frame(CAN_ERR_CRTL | CAN_ERR_CNT | CAN_ERR_BUSOFF, data));

        assert!(err.is_bus_off());
        assert!(err.has_counters());
        assert!(!err.is_no_ack());
        assert_eq!(err.counters(), Some((96, 0)));
        assert_eq!(
            err.controller(),
            Some(ControllerProblems::RX_WARNING | ControllerProblems::TX_WARNING)
        );
        assert_eq!(err.protocol(), None);
    }

    // ----- Display -----

    #[test]
    fn display_single_is_bare() {
        let err = CanError::new(ErrorCause::BusOff);
        assert_eq!(err.to_string(), "bus off");
    }

    #[test]
    fn display_multi_is_semicolon_joined() {
        let mut data = [0u8; 8];
        data[1] = 0x0C;
        data[6] = 96;
        data[7] = 0;
        let err = CanError::from(frame(CAN_ERR_CRTL | CAN_ERR_CNT, data));
        assert_eq!(
            err.to_string(),
            "controller problem: ERROR WARNING (receive), ERROR WARNING (transmit); \
             error counters: tx=96, rx=0"
        );
    }

    #[test]
    fn display_violations_at_one_location() {
        let mut data = [0u8; 8];
        data[2] = 0x9E;
        data[3] = 0x08;
        let err = CanError::from(frame(CAN_ERR_PROT | CAN_ERR_BUSERROR, data));
        assert_eq!(
            err.to_string(),
            "protocol violation at CRC sequence: frame format error, \
             bit stuffing error, unable to send dominant bit, \
             unable to send recessive bit, error on transmission; bus error"
        );
    }

    #[test]
    fn display_unknown_in_hex() {
        let err = CanError::new(ErrorCause::Unknown(0x400));
        assert_eq!(err.to_string(), "unknown error (0x400)");
    }

    // ----- iteration / conversion plumbing -----

    #[test]
    fn iteration_by_value_and_by_ref() {
        let err = CanError::from_multiple(
            ErrorCause::BusOff,
            [ErrorCause::NoAck, ErrorCause::Restarted],
        );
        assert_eq!(err.len(), 3);
        assert!(!err.is_single());
        assert_eq!(*err.last(), ErrorCause::Restarted);

        let by_ref: Vec<_> = (&err).into_iter().copied().collect();
        let by_val: Vec<_> = err.clone().into_iter().collect();
        assert_eq!(by_ref, by_val);
        assert_eq!(by_ref.len(), 3);

        let via_iter: Vec<_> = err.causes().copied().collect();
        assert_eq!(via_iter, by_val);
    }

    #[test]
    fn single_cause_promotes_to_error() {
        let err: CanError = ErrorCause::BusOff.into();
        assert!(err.is_single());

        // ... and through the top-level Error.
        let err: Error = ErrorCause::BusOff.into();
        match err {
            Error::Can(err) => assert_eq!(*err.first(), ErrorCause::BusOff),
            _ => panic!("expected a CAN error"),
        }
    }

    #[test]
    fn from_iter_checked_rejects_empty() {
        assert!(CanError::from_iter_checked(std::iter::empty()).is_none());
        assert!(CanError::from_iter_checked([ErrorCause::BusOff]).is_some());
    }
}
