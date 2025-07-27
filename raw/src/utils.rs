/// Gets a byte slice for any sized variable.
///
/// Note that this should normally be unsafe, but since we're only
/// using it internally for types sent to the kernel, it's OK.
pub fn as_bytes<T: Sized>(val: &T) -> &[u8] {
    let sz = size_of::<T>();
    unsafe { std::slice::from_raw_parts::<'_, u8>(val as *const _ as *const u8, sz) }
}

/// Gets a mutable byte slice for any sized variable.
pub fn as_bytes_mut<T: Sized>(val: &mut T) -> &mut [u8] {
    let sz = size_of::<T>();
    unsafe { std::slice::from_raw_parts_mut(val as *mut _ as *mut u8, sz) }
}
