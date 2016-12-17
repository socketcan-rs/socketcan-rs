use libc::timespec;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[inline]
pub fn duration_from_timeval(ts: timespec) -> Duration {
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

#[inline]
pub fn system_time_from_timespec(ts: timespec) -> SystemTime {
    UNIX_EPOCH + duration_from_timeval(ts)
}
