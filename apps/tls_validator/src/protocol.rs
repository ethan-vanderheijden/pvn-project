use anyhow::Result;
use pnet::packet::{
    Packet,
    ip::IpNextHeaderProtocols,
    tcp::{MutableTcpPacket, TcpFlags, TcpPacket},
    util,
};
use rustls::internal::msgs::{
    codec::Codec, handshake::HandshakeMessagePayload, handshake::HandshakePayload,
};
use std::net::IpAddr;

use crate::tcp_buffer::TcpBuffer;

pub const TLS_HEADER_LENGTH: u16 = 5;
pub const TLS_MAX_RECORD_LENGTH: u16 = 16384;

const TLS_HANDSHAKE_RECORD: u8 = 0x16;

/// Represents a TLS handshake frame as parsed from the TCP stream.
/// `is_tls12` indicates whether the frame says it is TLS v1.2 (but TLS v1.3
/// rides in TLS v1.2 frames).
pub struct HandshakeMessage<'a> {
    pub is_tls12: bool,
    pub total_len: usize,
    pub payload: HandshakePayload<'a>,
}

/// Parses the TLS handshake frame header from the given data and returns
/// the advertised TLS version and record length. Fails if it detects that
/// it isn't a TLS record or it isn't TLS v1.2.
fn read_handshake_frame(data: &[u8]) -> Result<(u8, u16), ()> {
    let record_type = data[0];
    let tls_major_version = data[1];
    let tls_minor_version = data[2];
    let length = ((data[3] as u16) << 8) | (data[4] as u16);
    // Some clients send ClientHello with TLS v1.0 frame for compatibility with old servers
    // In TLS v1.3, data is encapsulated with TLS v1.2 records
    // For these reasons, be very careful with how you use tls_minor_version
    if record_type != TLS_HANDSHAKE_RECORD
        || tls_major_version != 0x03
        || length > TLS_MAX_RECORD_LENGTH
    {
        Err(())
    } else {
        Ok((tls_minor_version, length))
    }
}

/// Reads a TLS handshake message from the TCP buffer. Returns None if there is not
/// enough data buffered yet. Returns an error if it can't be parsed as a TLS handshake.
pub fn read_handshake_msg(buffer: &TcpBuffer) -> Result<Option<HandshakeMessage>, ()> {
    if buffer.len() < TLS_HEADER_LENGTH as usize {
        // not enough data buffered yet
        return Ok(None);
    }

    let Ok((minor_version, record_length)) = read_handshake_frame(buffer.get_data()) else {
        // probably not a TLS flow
        return Err(());
    };

    if (buffer.get_data().len() - TLS_HEADER_LENGTH as usize) < (record_length as usize) {
        // not enough data buffered yet
        return Ok(None);
    }

    let start = TLS_HEADER_LENGTH as usize;
    let end = (TLS_HEADER_LENGTH + record_length) as usize;
    let Ok(handshake) = HandshakeMessagePayload::read_bytes(&buffer.get_data()[start..end]) else {
        // TLS handshake frame should be complete
        // so if handshake failed to decode, this probably isn't a real TLS flow
        return Err(());
    };
    Ok(Some(HandshakeMessage {
        is_tls12: minor_version == 3, // since 3.3 corresponds to TLSv1.2
        total_len: end,
        payload: handshake.payload,
    }))
}

/// Creates an empty TCP packet going in the opposite direction of `original`.
pub fn generate_return_rst(original: &TcpPacket, seqno: u32) -> MutableTcpPacket<'static> {
    let packet_size = TcpPacket::minimum_packet_size();
    let buffer = vec![0; packet_size];
    let mut packet = MutableTcpPacket::owned(buffer).unwrap();
    packet.set_destination(original.get_source());
    packet.set_source(original.get_destination());
    packet.set_sequence(seqno);
    packet.set_flags(TcpFlags::RST);
    packet.set_data_offset((packet_size / 4) as u8);
    packet
}

/// Calculates the TCP checksum for the given packet using `addr_1`` and `addr_2`.
/// Note: which address is source and destination does not matter for checksum calculation
pub fn set_tcp_checksum(packet: &mut MutableTcpPacket<'_>, addr_1: IpAddr, addr_2: IpAddr) {
    match (addr_1, addr_2) {
        (IpAddr::V4(addr_1), IpAddr::V4(addr_2)) => {
            packet.set_checksum(util::ipv4_checksum(
                packet.packet(),
                8, // skip checksum field during checksum calculation
                &[],
                &addr_1,
                &addr_2,
                IpNextHeaderProtocols::Tcp,
            ));
        }
        (IpAddr::V6(addr_1), IpAddr::V6(addr_2)) => {
            packet.set_checksum(util::ipv6_checksum(
                packet.packet(),
                8,
                &[],
                &addr_1,
                &addr_2,
                IpNextHeaderProtocols::Tcp,
            ));
        }
        _ => {}
    }
}
