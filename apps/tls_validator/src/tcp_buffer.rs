use std::fmt::Debug;

use pnet::packet::{tcp::TcpPacket, Packet};

/// Represents an out-of-order segment in the TCP buffer.
/// `start` is the offset from the start of the buffer (not
/// the sequence number).
#[derive(Debug)]
struct Segment {
    start: usize,
    len: usize,
}

/// Implements a TCP buffer that supports storing out of order segments.
pub struct TcpBuffer {
    buf: Vec<u8>,
    max_capacity: usize,
    initial_seqno: u32,
    valid_data: usize,
    received_segments: Vec<Segment>,
}

impl TcpBuffer {
    /// Creates an empty TCP buffer that can hold up to `capacity` bytes of data.
    /// The `initial_seqno` is the expected sequence number of the next packet.
    pub fn new(initial_seqno: u32, capacity: usize) -> TcpBuffer {
        TcpBuffer {
            buf: Vec::new(),
            max_capacity: capacity,
            initial_seqno,
            valid_data: 0,
            received_segments: Vec::new(),
        }
    }

    /// Process as many segments as possible that are contiguous with the beginning
    /// of the buffer. These segments are deleted and the data they refer to are
    /// marked as good data.
    fn process_valid_data(&mut self) {
        let mut keep_iterating = true;
        while keep_iterating {
            keep_iterating = false;
            for segment in &self.received_segments {
                if segment.start == self.valid_data {
                    self.valid_data += segment.len;
                    keep_iterating = true;
                    break;
                }
            }
        }
        self.received_segments
            .retain(|segment| segment.start > self.valid_data);
    }

    /// Add data from a TCP packet to the buffer. The packet's sequence number can be out
    /// of order.
    pub fn add_packet_data(&mut self, packet: &TcpPacket)
    {
        let payload = packet.payload();
        if payload.len() > 0 {
            self.add_segment(packet.get_sequence(), payload);
        }
    }

    /// Data bytes of data the buffer with the given sequence number. If the data
    /// is outside of `max_capacity`, it will is ignored. If within `max_capacity`,
    /// it is injected into the buffer at the appropriate offset and `received_segments`
    /// is updated. New good segments are processed thereafter.
    fn add_segment(&mut self, seqno: u32, mut data: &[u8]) {
        let offset;
        if seqno < self.initial_seqno {
            offset = (u32::MAX - self.initial_seqno + seqno) as usize;
        } else {
            offset = (seqno - self.initial_seqno) as usize;
        }

        if offset >= self.max_capacity {
            return;
        }

        let mut end = offset + data.len();
        if end > self.max_capacity {
            data = &data[0..(self.max_capacity - offset)];
            end = self.max_capacity;
        }

        self.received_segments
            .retain(|segment| end <= segment.start || segment.start + segment.len <= offset);

        if end > self.buf.len() {
            self.buf.resize(end, 0);
        }
        self.buf[offset..end].copy_from_slice(data);
        self.received_segments.push(Segment {
            start: offset,
            len: data.len(),
        });
        self.process_valid_data();
    }

    /// Amount of good data that can be immediately read from the buffer.
    pub fn len(&self) -> usize {
        self.valid_data
    }

    /// Returns the good data currently in the buffer.
    pub fn get_data(&self) -> &[u8] {
        &self.buf[0..self.valid_data]
    }

    /// Drains the first `length` bytes from the buffer. `length` can be larger
    /// than the amount of good data, in which case it will skip the excess bytes
    /// received in future packets.
    pub fn drain(&mut self, length: usize) {
        self.initial_seqno += length as u32;
        self.buf.drain(..length);
        if self.valid_data > length {
            self.valid_data -= length;
        } else {
            self.valid_data = 0;
        }

        self.received_segments = self
            .received_segments
            .iter()
            .filter_map(|ele| {
                if ele.start < length {
                    None
                } else {
                    Some(Segment {
                        start: ele.start - length,
                        len: ele.len,
                    })
                }
            })
            .collect();
        self.process_valid_data();
    }
}

impl Debug for TcpBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpBuffer")
            .field("initial_seqno", &self.initial_seqno)
            .field("valid_data", &self.valid_data)
            .field("received_segments", &self.received_segments)
            .field("buffer_size", &self.buf.len())
            .finish()
    }
}
