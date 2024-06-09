// socketcan/examples/tokio_average.rs
//
// Example application for using Tokio with socketcan-rs.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! A SocketCAN example using Tokio.
//!
//! This receives CAN data frames of 32-bit integer values (Little Endian),
//! performs a running average on them, and output the average on a different
//! CAN ID on the same bus.
//!
//! Note that this is an overly complex implementation for what it is doing,
//! but serves as an example of multiple tasks using the library, and also
//! can run indefinitely as a life test to insure no bugs arise over
//! days/weeks/months of continuous use.
//!

use futures_util::StreamExt;
use socketcan::{
    tokio::CanSocket, CanFilter, CanFrame, EmbeddedFrame, Error, Frame, Result, SocketOptions,
    StandardId,
};
use std::collections::VecDeque;
use tokio::sync::mpsc;

struct MovingAverage {
    sum: i32,
    data: VecDeque<i32>,
}

impl MovingAverage {
    pub fn new(max_n: usize) -> Self {
        let mut data = VecDeque::with_capacity(max_n);
        (0..max_n).for_each(|_| data.push_front(0));

        Self { sum: 0, data }
    }

    pub fn avg(&mut self, pt: i32) -> i32 {
        let old_pt = self.data.pop_back().unwrap();
        self.sum = self.sum + pt - old_pt;
        self.data.push_front(pt);
        self.sum / self.data.len() as i32
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut sock_rx = CanSocket::open("vcan0")?;
    let sock_tx = CanSocket::open("vcan0")?;

    sock_rx.set_filters(&[CanFilter::new(0x100, 0x7FF)])?;

    let (tx, mut rx) = mpsc::channel::<CanFrame>(3);

    tokio::spawn(async move {
        let mut data = MovingAverage::new(5);

        while let Some(mut frame) = rx.recv().await {
            let n = i32::from_le_bytes(frame.data()[0..4].try_into().unwrap());
            let avg = data.avg(n);

            frame.set_id(StandardId::new(0x101).unwrap());
            frame.set_data(&avg.to_le_bytes()).unwrap();

            sock_tx.write_frame(frame).await?;
        }

        Ok::<(), Error>(())
    });

    while let Some(Ok(frame)) = sock_rx.next().await {
        if matches!(frame, CanFrame::Data(_)) {
            tx.send(frame).await.unwrap();
        }
    }

    Ok(())
}
