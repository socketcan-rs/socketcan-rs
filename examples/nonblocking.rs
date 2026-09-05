// socketcan/examples/nonblocking.rs
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//
// @author Natesh Narain <nnaraindev@gmail.com>
// @date Jul 05 2022
//

//! Reading and writing a non-blocking socket.
//!
//! A non-blocking socket answers `WouldBlock` instead of parking the thread,
//! so the caller keeps control: it can service other work between attempts,
//! and can give up on its own schedule. That is the whole point of the mode,
//! so this example handles `WouldBlock` itself rather than handing the frame
//! to `nb::block!`, which would spin until one arrived and put the thread
//! right back where a blocking socket would have left it.
//!
//! For an event-driven program, prefer the `tokio` or `smol` wrappers, which
//! wait on readiness rather than polling.

use anyhow::{Context, bail};
use embedded_can::{Frame as EmbeddedFrame, StandardId, nb::Can};
use socketcan::{CanFrame, CanSocket, Frame, Socket};
use std::{
    env,
    thread::sleep,
    time::{Duration, Instant},
};

/// How long to wait for a frame before giving up.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait between attempts. Without it the loop would spin a core.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

// --------------------------------------------------------------------------

fn frame_to_string<F: Frame>(frame: &F) -> String {
    let id = frame.raw_id();
    let data_string = frame
        .data()
        .iter()
        .fold(String::from(""), |a, b| format!("{} {:02x}", a, b));

    format!("{:X}  [{}] {}", id, frame.dlc(), data_string)
}

// --------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "can0".into());

    let mut sock = CanSocket::open(&iface)
        .with_context(|| format!("Failed to open socket on interface {}", iface))?;

    sock.set_nonblocking(true)
        .context("Failed to make socket non-blocking")?;

    // Wait for a frame, but on our own terms: `receive()` returns immediately
    // either way, so the application decides what to do with the time and when
    // to stop waiting. A blocking socket can do neither without `SO_RCVTIMEO`.
    println!("Waiting up to {:?} for a frame on {}...", TIMEOUT, iface);
    let deadline = Instant::now() + TIMEOUT;
    let mut polls: u64 = 0;

    let frame = loop {
        match sock.receive() {
            Ok(frame) => break frame,
            Err(nb::Error::Other(err)) => return Err(err).context("Receiving frame"),
            Err(nb::Error::WouldBlock) => {
                if Instant::now() >= deadline {
                    bail!("No frame received on {} within {:?}", iface, TIMEOUT);
                }
                // Where an application would service its other work.
                polls += 1;
                sleep(POLL_INTERVAL);
            }
        }
    };

    println!("{}  {}", iface, frame_to_string(&frame));
    println!("  (idle through {} polls while waiting)", polls);

    let frame = CanFrame::new(StandardId::new(0x1f1).unwrap(), &[1, 2, 3, 4])
        .context("Creating CAN frame")?;

    // Sending can report `WouldBlock` too, when the socket's send buffer is
    // full — a slow bus with a fast writer. Retry until it goes out, or until
    // the same deadline passes.
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match sock.transmit(&frame) {
            Ok(_) => break,
            Err(nb::Error::Other(err)) => return Err(err).context("Transmitting frame"),
            Err(nb::Error::WouldBlock) => {
                if Instant::now() >= deadline {
                    bail!("Send buffer stayed full for {:?}", TIMEOUT);
                }
                sleep(POLL_INTERVAL);
            }
        }
    }

    println!("Sent: {}", frame_to_string(&frame));
    Ok(())
}
