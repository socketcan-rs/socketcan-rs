pub(crate) mod addr;
pub(crate) mod socket;
pub(crate) mod utils;

pub use self::{addr::*, socket::*, utils::*};

// ===== AsPtr trait =====

/// Trait to get a pointer to an inner type
pub trait AsPtr {
    /// The inner type to which we resolve as a pointer
    type Inner;

    /// Gets a const pointer to the inner type
    fn as_ptr(&self) -> *const Self::Inner;

    /// Gets a mutable pointer to the inner type
    fn as_mut_ptr(&mut self) -> *mut Self::Inner;

    /// The size of the inner type
    fn size(&self) -> usize {
        size_of::<Self::Inner>()
    }

    /// Gets a byte slice to the inner type
    fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts::<'_, u8>(
                self.as_ptr() as *const _ as *const u8,
                self.size(),
            )
        }
    }

    /// Gets a mutable byte slice to the inner type
    fn as_bytes_mut(&mut self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts::<'_, u8>(
                self.as_mut_ptr() as *mut _ as *mut u8,
                self.size(),
            )
        }
    }
}
