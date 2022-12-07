use embedded_can::Id;
use libc::{
    c_int,
    c_void,
    setsockopt,
    socklen_t,
    //timespec, timeval, time_t, suseconds_t
};
use std::{
    io,
    mem::size_of,
    //time::{Duration, SystemTime, UNIX_EPOCH},
    ptr,
};

/// `setsockopt` wrapper
///
/// The libc `setsockopt` function is set to set various options on a socket.
/// `set_socket_option` offers a somewhat type-safe wrapper that does not
/// require messing around with `*const c_void`s.
///
/// A proper `std::io::Error` will be returned on failure.
///
/// Example use:
///
/// ```text
/// let fd = ...;  // some file descriptor, this will be stdout
/// set_socket_option(fd, SOL_TCP, TCP_NO_DELAY, 1 as c_int)
/// ```
///
/// Note that the `val` parameter must be specified correctly; if an option
/// expects an integer, it is advisable to pass in a `c_int`, not the default
/// of `i32`.
#[inline]
pub fn set_socket_option<T>(fd: c_int, level: c_int, name: c_int, val: &T) -> io::Result<()> {
    let rv = unsafe {
        setsockopt(
            fd,
            level,
            name,
            val as *const _ as *const c_void,
            size_of::<T>() as socklen_t,
        )
    };

    if rv != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

pub fn set_socket_option_mult<T>(
    fd: c_int,
    level: c_int,
    name: c_int,
    values: &[T],
) -> io::Result<()> {
    let rv = if values.is_empty() {
        // can't pass in a ptr to a 0-len slice, pass a null ptr instead
        unsafe { setsockopt(fd, level, name, ptr::null(), 0) }
    } else {
        unsafe {
            setsockopt(
                fd,
                level,
                name,
                values.as_ptr() as *const c_void,
                (size_of::<T>() * values.len()) as socklen_t,
            )
        }
    };

    if rv != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

/*
pub fn c_timeval_new(t: time::Duration) -> timeval {
    timeval {
        tv_sec: t.as_secs() as time_t,
        tv_usec: (t.subsec_nanos() / 1000) as suseconds_t,
    }
}

#[inline]
pub fn duration_from_timeval(ts: timespec) -> Duration {
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

#[inline]
pub fn system_time_from_timespec(ts: timespec) -> SystemTime {
    UNIX_EPOCH + duration_from_timeval(ts)
}
*/

pub fn hal_id_to_raw(id: Id) -> u32 {
    match id {
        Id::Standard(id) => id.as_raw() as u32,
        Id::Extended(id) => id.as_raw(),
    }
}
