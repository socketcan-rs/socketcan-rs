use crate::err::{CanError, CanErrorDecodingFailure, ConstructionError};
use crate::util::hal_id_to_raw;
use embedded_hal::can::{ExtendedId, Frame as EmbeddedFrame, Id, StandardId};

use std::{convert::TryFrom, fmt, mem};

use itertools::Itertools;

/// if set, indicate 29 bit extended format
pub const EFF_FLAG: u32 = 0x80000000;

/// remote transmission request flag
pub const RTR_FLAG: u32 = 0x40000000;

/// error flag
pub const ERR_FLAG: u32 = 0x20000000;

/// valid bits in standard frame id
pub const SFF_MASK: u32 = 0x000007ff;

/// valid bits in extended frame id
pub const EFF_MASK: u32 = 0x1fffffff;

/// valid bits in error frame
pub const ERR_MASK: u32 = 0x1fffffff;

/// an error mask that will cause SocketCAN to report all errors
pub const ERR_MASK_ALL: u32 = ERR_MASK;

/// an error mask that will cause SocketCAN to silently drop all errors
pub const ERR_MASK_NONE: u32 = 0;

/// 'legacy' CAN frame
pub const CAN_DATA_LEN_MAX: usize = 8;

/// CAN FD frame
pub const CANFD_DATA_LEN_MAX: usize = 64;

/// CAN FD flags
pub const CANFD_BRS: u8 = 0x01; /* bit rate switch (second bitrate for payload data) */
pub const CANFD_ESI: u8 = 0x02; /* error state indicator of the transmitting node */

fn init_raw_id(id: u32, ext_id: bool, rtr: bool, err: bool) -> Result<u32, ConstructionError> {
    let mut _id = id;

    if id > EFF_MASK {
        return Err(ConstructionError::IDTooLarge);
    }

    if ext_id || id > SFF_MASK {
        _id |= EFF_FLAG;
    }

    if rtr {
        _id |= RTR_FLAG;
    }

    if err {
        _id |= ERR_FLAG;
    }

    Ok(_id)
}

fn is_extended(id: &Id) -> bool {
    match id {
        Id::Standard(_) => false,
        Id::Extended(_) => true,
    }
}

pub trait Frame: EmbeddedFrame {
    /// Get the full SocketCAN ID word (with EFF/RTR/ERR flags)
    fn id_word(&self) -> u32;

    /// Return the actual raw CAN ID (without EFF/RTR/ERR flags)
    fn raw_id(&self) -> u32 {
        // TODO: Standard use SFF mask, or is this OK?
        self.id_word() & EFF_MASK
    }

    /// Return the CAN ID as the embedded HAL Id type.
    fn hal_id(&self) -> Id {
        if self.is_extended() {
            Id::Extended(ExtendedId::new(self.id_word() & EFF_MASK).unwrap())
        } else {
            Id::Standard(StandardId::new((self.id_word() & SFF_MASK) as u16).unwrap())
        }
    }

    /// Get the data length
    fn len(&self) -> usize {
        self.dlc()
    }

    /// Return the error message
    fn err(&self) -> u32 {
        self.id_word() & ERR_MASK
    }

    /// Check if frame is an error message
    fn is_error(&self) -> bool {
        self.id_word() & ERR_FLAG != 0
    }

    fn error(&self) -> Result<CanError, CanErrorDecodingFailure>
    where
        Self: Sized,
    {
        CanError::from_frame(self)
    }
}

// ===== CanAnyFrame =====

pub enum CanAnyFrame {
    Normal(CanNormalFrame),
    Fd(CanFdFrame),
}

impl fmt::Debug for CanAnyFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Normal(frame) => {
                write!(f, "CAN Frame {:?}", frame)
            }

            Self::Fd(frame) => {
                write!(f, "CAN FD Frame {:?}", frame)
            }
        }
    }
}

// ===== CanNormalFrame =====

/// Uses the same memory layout as the underlying kernel struct for performance
/// reasons.
#[derive(Debug, Copy, Clone)]
#[repr(C, align(8))]
pub struct CanNormalFrame {
    /// 32 bit CAN_ID + EFF/RTR/ERR flags
    _id: u32,

    /// data length. Bytes beyond are not valid
    _data_len: u8,

    /// padding
    _pad: u8,

    /// reserved
    _res0: u8,

    /// reserved
    _res1: u8,

    /// buffer for data
    _data: [u8; CAN_DATA_LEN_MAX],
}

impl CanNormalFrame {
    pub fn init(
        id: u32,
        data: &[u8],
        ext_id: bool,
        rtr: bool,
        err: bool,
    ) -> Result<Self, ConstructionError> {
        let n = data.len();

        if n > CAN_DATA_LEN_MAX {
            return Err(ConstructionError::TooMuchData);
        }

        let mut _id = init_raw_id(id, ext_id, rtr, err)?;

        let mut _data = [0u8; CAN_DATA_LEN_MAX];
        (&mut _data[..n]).copy_from_slice(data);

        Ok(Self {
            _id,
            _data_len: n as u8,
            _data,
            ..Self::default()
        })
    }
}

impl EmbeddedFrame for CanNormalFrame {
    /// Create a new frame
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        let id = id.into();
        let is_ext = is_extended(&id);
        let raw_id = hal_id_to_raw(id);
        Self::init(raw_id, data, is_ext, false, false).ok()
    }

    /// Create a new remote transmission request frame.
    fn new_remote(id: impl Into<Id>, dlc: usize) -> Option<Self> {
        let id = id.into();
        let is_ext = is_extended(&id);
        let raw_id = hal_id_to_raw(id);
        let data = [0u8; 8];
        Self::init(raw_id, &data[0..dlc], is_ext, true, false).ok()
    }

    /// Check if frame uses 29 bit extended frame format
    fn is_extended(&self) -> bool {
        self._id & EFF_FLAG != 0
    }

    /// Check if frame is a remote transmission request.
    fn is_remote_frame(&self) -> bool {
        self._id & RTR_FLAG != 0
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        self.hal_id()
    }

    /// Data length
    /// TODO: Return the proper DLC code for remote frames?
    fn dlc(&self) -> usize {
        self._data_len as usize
    }

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    fn data(&self) -> &[u8] {
        &self._data[..(self._data_len as usize)]
    }
}

impl Frame for CanNormalFrame {
    fn id_word(&self) -> u32 {
        self._id
    }
}

impl Default for CanNormalFrame {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl TryFrom<CanFdFrame> for CanNormalFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanFdFrame) -> Result<Self, Self::Error> {
        if frame._data_len > CAN_DATA_LEN_MAX as u8 {
            return Err(ConstructionError::TooMuchData);
        }

        CanNormalFrame::init(
            frame.raw_id(),
            &frame.data()[..(frame._data_len as usize)],
            frame.is_extended(),
            false,
            frame.is_error(),
        )
    }
}

// ===== CanFdFrame =====

/// Uses the same memory layout as the underlying kernel struct for performance
/// reasons.
#[derive(Debug, Copy, Clone)]
#[repr(C, align(8))]
pub struct CanFdFrame {
    /// 32 bit CAN_ID + EFF/RTR/ERR flags
    _id: u32,

    /// data length. Bytes beyond are not valid
    _data_len: u8,

    /// flags for CAN FD
    _flags: u8,

    /// reserved
    _res0: u8,

    /// reserved
    _res1: u8,

    /// buffer for data
    _data: [u8; CANFD_DATA_LEN_MAX],
}

impl CanFdFrame {
    pub fn init(
        id: u32,
        data: &[u8],
        ext_id: bool,
        err: bool,
        brs: bool,
        esi: bool,
    ) -> Result<Self, ConstructionError> {
        let n = data.len();

        if n > CAN_DATA_LEN_MAX {
            return Err(ConstructionError::TooMuchData);
        }

        let mut _id = init_raw_id(id, ext_id, false, err)?;

        let mut flags: u8 = 0;
        if brs {
            flags = flags | CANFD_BRS;
        }
        if esi {
            flags = flags | CANFD_ESI;
        }

        let mut _data = [0u8; CANFD_DATA_LEN_MAX];
        (&mut _data[..n]).copy_from_slice(data);

        Ok(Self {
            _id,
            _data_len: n as u8,
            _flags: flags,
            _data,
            ..Self::default()
        })
    }

    pub fn is_brs(&self) -> bool {
        self._flags & CANFD_BRS == CANFD_BRS
    }

    pub fn set_brs(&mut self, on: bool) {
        if on {
            self._flags |= CANFD_BRS;
        } else {
            self._flags &= !CANFD_BRS;
        }
    }

    pub fn is_esi(&self) -> bool {
        self._flags & CANFD_ESI == CANFD_ESI
    }

    pub fn set_esi(&mut self, on: bool) {
        if on {
            self._flags |= CANFD_ESI;
        } else {
            self._flags &= !CANFD_ESI;
        }
    }
}

impl EmbeddedFrame for CanFdFrame {
    /// Create a new frame
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        let id = id.into();
        let is_ext = is_extended(&id);
        let raw_id = hal_id_to_raw(id);
        Self::init(raw_id, data, is_ext, false, false, false).ok()
    }

    /// CAN FD frames don't support remote
    fn new_remote(_id: impl Into<Id>, _dlc: usize) -> Option<Self> {
        None
    }

    /// Check if frame uses 29 bit extended frame format
    fn is_extended(&self) -> bool {
        self._id & EFF_FLAG != 0
    }

    /// The FD frames don't support remote request
    fn is_remote_frame(&self) -> bool {
        false
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        self.hal_id()
    }

    /// Data length
    fn dlc(&self) -> usize {
        self._data_len as usize
    }

    /// A slice into the actual data.
    ///
    /// For normal CAN frames the slice will always be <= 8 bytes in length.
    fn data(&self) -> &[u8] {
        &self._data[..(self._data_len as usize)]
    }
}

impl Frame for CanFdFrame {
    fn id_word(&self) -> u32 {
        self._id
    }
}

impl Default for CanFdFrame {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl From<CanNormalFrame> for CanFdFrame {
    fn from(frame: CanNormalFrame) -> Self {
        CanFdFrame {
            _id: frame._id,
            _data_len: frame.data().len() as u8,
            _flags: 0,
            _res0: 0,
            _res1: 0,
            _data: slice_to_array::<CANFD_DATA_LEN_MAX>(frame.data()),
        }
    }
}

impl From<CanNormalFrame> for CanAnyFrame {
    fn from(frame: CanNormalFrame) -> Self {
        CanAnyFrame::Normal(frame)
    }
}

impl From<CanFdFrame> for CanAnyFrame {
    fn from(frame: CanFdFrame) -> Self {
        CanAnyFrame::Fd(frame)
    }
}

fn slice_to_array<const S: usize>(data: &[u8]) -> [u8; S] {
    let mut array = [0; S];

    for (i, b) in data.iter().enumerate() {
        array[i] = *b;
    }
    array
}

impl fmt::UpperHex for CanNormalFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}{}", self._id, "#")?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        let sep = if f.alternate() { " " } else { " " };
        write!(f, "{}", parts.join(sep))
    }
}

impl fmt::UpperHex for CanFdFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}{}", self._id, "##")?;
        write!(f, "{} ", self._flags)?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        let sep = if f.alternate() { " " } else { " " };
        write!(f, "{}", parts.join(sep))
    }
}

impl fmt::UpperHex for CanAnyFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal(frame) => frame.fmt(f),
            Self::Fd(frame) => frame.fmt(f),
        }
    }
}
