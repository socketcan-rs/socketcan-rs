// socketcan/examples/can_recvfd.rs
//
// Example: receive frames on a CAN FD socket and print them. A single
// `CanFdSocket` delivers classic CAN 2.0 frames, CAN FD frames, and error
// frames alike, so this one loop handles all three. No timestamps are
// requested; see `can_recvts` for the timestamped variant.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//
// Usage:
//   can_recvfd [interface]     (default interface: can0)

use socketcan::{CanAnyFrame, CanFdSocket, Frame, Socket, SocketOptions, id::ERR_MASK_ALL};
use std::env;

// --------------------------------------------------------------------------

/// Renders the identifier, length and data bytes common to every frame type.
fn frame_info<F: Frame>(frame: &F) -> String {
    let id = frame.raw_id();
    let data = frame
        .data()
        .iter()
        .fold(String::new(), |s, b| format!("{s} {b:02X}"));
    format!("{id:08X}  [{}]{data}", frame.data().len())
}

/// Renders a received frame for display, tagged by kind.
///
/// A `CanFdSocket` yields a [`CanAnyFrame`], so a read can produce a classic
/// data frame, a remote frame, a CAN FD frame, or an error frame. An error
/// frame is not bus traffic — it is the driver reporting a problem — so it is
/// shown as the decoded error text rather than as raw bytes. A single error
/// frame can report several causes at once, and `CanError` renders all of
/// them on one line.
fn frame_str(frame: &CanAnyFrame) -> String {
    match frame {
        CanAnyFrame::Normal(frame) => format!("CAN    {}", frame_info(frame)),
        CanAnyFrame::Remote(frame) => format!("RTR    {}", frame_info(frame)),
        CanAnyFrame::Fd(frame) => {
            // Note the bit-rate-switch and error-state-indicator flags.
            let brs = if frame.is_brs() { 'B' } else { '-' };
            let esi = if frame.is_esi() { 'E' } else { '-' };
            format!("FD {brs}{esi}  {}", frame_info(frame))
        }
        CanAnyFrame::Error(frame) => format!("ERROR: {}", frame.into_error()),
    }
}

// --------------------------------------------------------------------------

fn main() -> std::io::Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "can0".to_string());

    // Opening an FD socket enables reception of both classic and FD frames.
    let sock = CanFdSocket::open(&iface)?;

    // Error frames are not delivered unless asked for.
    sock.set_error_mask(ERR_MASK_ALL)?;

    println!("Reading CAN / CAN FD / error frames on {iface}");
    loop {
        let frame = sock.read_frame()?;
        println!("{}", frame_str(&frame));
    }
}
