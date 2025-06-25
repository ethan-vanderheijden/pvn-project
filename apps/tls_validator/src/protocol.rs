use anyhow::Result;
use pnet::{
    packet::{
        Packet,
        tcp::{MutableTcpPacket, TcpFlags, TcpPacket},
    },
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

pub struct HandshakeMessage<'a> {
    pub is_tls12: bool,
    pub total_len: usize,
    pub payload: HandshakePayload<'a>,
}

fn read_handshake_frame(data: &[u8]) -> Result<(u8, u16), ()> {
    let record_type = data[0];
    let tls_major_version = data[1];
    let tls_minor_version = data[2];
    let length = ((data[3] as u16) << 8) | (data[4] as u16);
    // Some clients send ClientHello with TLS v1.0 frame for compatibility with old servers
    // In TLS v1.3, data is encapsulated with TLS v1.2 records
    // For these reasons, be very careful with how you use tls_minor_version
    if record_type != TLS_HANDSHAKE_RECORD || tls_major_version != 0x03 || length > 16384 {
        Err(())
    } else {
        Ok((tls_minor_version, length))
    }
}

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

fn ipaddr_bytes(ip: IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(ipv4_addr) => ipv4_addr.octets().to_vec(),
        IpAddr::V6(ipv6_addr) => ipv6_addr.octets().to_vec(),
    }
}

pub fn set_tcp_checksum(packet: &mut MutableTcpPacket, src_ip: IpAddr, dest_ip: IpAddr) {
    let src_bytes = ipaddr_bytes(src_ip);
    let dest_bytes = ipaddr_bytes(dest_ip);
    let proto_type = &[0, 6u8];
    let packet_size = &[0, packet.packet().len() as u8];
    // Note: checksum field is at offset 16 in TCP header
    let checksum_word_offset =
        (src_bytes.len() + dest_bytes.len() + proto_type.len() + packet_size.len() + 16) / 2;
    let checksum_data = [
        src_bytes.as_slice(),
        dest_bytes.as_slice(),
        proto_type,
        packet_size,
        packet.packet(),
    ]
    .concat();
    let checksum = util::checksum(&checksum_data, checksum_word_offset);
    packet.set_checksum(checksum);
}

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
