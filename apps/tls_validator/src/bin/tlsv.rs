use anyhow::{Result, anyhow};
use pnet::{
    datalink::{self, Channel::Ethernet, ChannelType, Config, NetworkInterface},
    packet::{
        Packet,
        ip::IpNextHeaderProtocols,
        ipv4::{self, Ipv4Packet, MutableIpv4Packet},
        ipv6::{Ipv6Packet, MutableIpv6Packet},
        tcp::TcpPacket,
    },
};
use std::{
    env,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};
use tls_validator::{TcpFlow, TlsvMiddlebox, TlsvResult};
use tracing::Level;

const BUFFER_SIZE: usize = 16384;
const NEW_PACKET_TTL: u8 = 64;

/// Takes bytes, which must be a valid TCP packet, and builds an IPv4 packet with the
/// given source and destination addresses.
fn prepare_ipv4_packet(packet_buf: &mut [u8], src: Ipv4Addr, dest: Ipv4Addr, tcp: &TcpPacket) {
    let packet_len = packet_buf.len() as u16;
    let mut packet = MutableIpv4Packet::new(packet_buf).unwrap();
    packet.set_version(4);
    packet.set_source(src);
    packet.set_destination(dest);
    packet.set_next_level_protocol(IpNextHeaderProtocols::Tcp);
    packet.set_ttl(NEW_PACKET_TTL);
    packet.set_header_length((Ipv4Packet::minimum_packet_size() / 4) as u8);
    packet.set_total_length(packet_len);
    packet.set_payload(tcp.packet());
    packet.set_checksum(ipv4::checksum(&packet.to_immutable()));
}

/// Takes bytes, which must be a valid TCP packet, and builds an IPv6 packet with the
/// given source and destination addresses.
fn prepare_ipv6_packet(packet_buf: &mut [u8], src: Ipv6Addr, dest: Ipv6Addr, tcp: &TcpPacket) {
    let mut packet = MutableIpv6Packet::new(packet_buf).unwrap();
    packet.set_version(6);
    packet.set_source(src);
    packet.set_destination(dest);
    packet.set_next_header(IpNextHeaderProtocols::Tcp);
    packet.set_hop_limit(NEW_PACKET_TTL);
    packet.set_payload_length(tcp.packet().len() as u16);
    packet.set_payload(tcp.packet());
}

/// Listens for IPv4 + TCP packets coming into the interface and sends them to the
/// TLS Validator middlebox for processing. Forwards packets as directed by the middlebox.
fn process_ipv4(interface: &NetworkInterface, client_ip: Ipv4Addr) {
    let config = Config {
        write_buffer_size: BUFFER_SIZE,
        read_buffer_size: BUFFER_SIZE,
        channel_type: ChannelType::Layer3(0x0800 as u16),
        ..Default::default()
    };

    let Ok(Ethernet(mut tx, mut rx)) = datalink::channel(&interface, config) else {
        eprintln!("Couldn't find Ethernet channel.");
        return;
    };
    let mut middlebox = TlsvMiddlebox::new();

    loop {
        match rx.next() {
            Ok(packet) => {
                let ip_packet = Ipv4Packet::new(packet).unwrap();
                if matches!(
                    ip_packet.get_next_level_protocol(),
                    IpNextHeaderProtocols::Tcp
                ) {
                    let tcp_packet = TcpPacket::new(ip_packet.payload()).unwrap();
                    let flow = TcpFlow {
                        source_ip: IpAddr::V4(ip_packet.get_source()),
                        dest_ip: IpAddr::V4(ip_packet.get_destination()),
                        source_port: tcp_packet.get_source(),
                        dest_port: tcp_packet.get_destination(),
                    };
                    let result = if client_ip == ip_packet.get_source() {
                        Some(middlebox.process_outgoing(&tcp_packet, flow))
                    } else if client_ip == ip_packet.get_destination() {
                        Some(middlebox.process_incoming(&tcp_packet, flow.reverse()))
                    } else {
                        None
                    };

                    if let Some(TlsvResult::Invalid {
                        forward_packet,
                        return_packet,
                    }) = result
                    {
                        tx.build_and_send(
                            1,
                            Ipv4Packet::minimum_packet_size() + forward_packet.packet().len(),
                            &mut |buf| {
                                prepare_ipv4_packet(
                                    buf,
                                    ip_packet.get_source(),
                                    ip_packet.get_destination(),
                                    &forward_packet,
                                );
                            },
                        )
                        .expect("No buffer left")
                        .expect("Failed to send packet");
                        tx.build_and_send(
                            1,
                            Ipv4Packet::minimum_packet_size() + return_packet.packet().len(),
                            &mut |buf| {
                                prepare_ipv4_packet(
                                    buf,
                                    ip_packet.get_destination(),
                                    ip_packet.get_source(),
                                    &return_packet,
                                );
                            },
                        )
                        .expect("No buffer left")
                        .expect("Failed to send packet");
                        continue;
                    }
                }
                tx.send_to(packet, None);
            }
            Err(e) => {
                eprintln!("Error occured while reading packet: {}", e);
            }
        }
    }
}

/// Identical to `process_ipv4`, but for IPv6 packets.
fn process_ipv6(interface: &NetworkInterface, client_ip: Ipv6Addr) {
    let config = Config {
        write_buffer_size: BUFFER_SIZE,
        read_buffer_size: BUFFER_SIZE,
        channel_type: ChannelType::Layer3(0x08DD as u16),
        ..Default::default()
    };

    let Ok(Ethernet(mut tx, mut rx)) = datalink::channel(&interface, config) else {
        eprintln!("Couldn't find Ethernet channel.");
        return;
    };
    let mut middlebox = TlsvMiddlebox::new();

    loop {
        match rx.next() {
            Ok(packet) => {
                let ip_packet = Ipv6Packet::new(packet).unwrap();
                if matches!(ip_packet.get_next_header(), IpNextHeaderProtocols::Tcp) {
                    let tcp_packet = TcpPacket::new(ip_packet.payload()).unwrap();
                    let flow = TcpFlow {
                        source_ip: IpAddr::V6(ip_packet.get_source()),
                        dest_ip: IpAddr::V6(ip_packet.get_destination()),
                        source_port: tcp_packet.get_source(),
                        dest_port: tcp_packet.get_destination(),
                    };
                    let result = if client_ip == ip_packet.get_source() {
                        Some(middlebox.process_outgoing(&tcp_packet, flow))
                    } else if client_ip == ip_packet.get_destination() {
                        Some(middlebox.process_incoming(&tcp_packet, flow.reverse()))
                    } else {
                        None
                    };

                    if let Some(TlsvResult::Invalid {
                        forward_packet,
                        return_packet,
                    }) = result
                    {
                        tx.build_and_send(
                            1,
                            Ipv6Packet::minimum_packet_size() + forward_packet.packet().len(),
                            &mut |buf| {
                                prepare_ipv6_packet(
                                    buf,
                                    ip_packet.get_source(),
                                    ip_packet.get_destination(),
                                    &forward_packet,
                                );
                            },
                        )
                        .expect("No buffer left")
                        .expect("Failed to send packet");
                        tx.build_and_send(
                            1,
                            Ipv6Packet::minimum_packet_size() + return_packet.packet().len(),
                            &mut |buf| {
                                prepare_ipv6_packet(
                                    buf,
                                    ip_packet.get_destination(),
                                    ip_packet.get_source(),
                                    &return_packet,
                                );
                            },
                        )
                        .expect("No buffer left")
                        .expect("Failed to send packet");
                        continue;
                    }
                }
                tx.send_to(packet, None);
            }
            Err(e) => {
                eprintln!("Error occured while reading packet: {}", e);
            }
        }
    }
}

/// The TLS Validator silently listens to TCP packets traversing to/from the end user
/// and sniffs out the server's TLS certificate. The middlebox performs its own cert
/// validation, and if the certificate is invalid, it disrupts the connection with
/// RST packets.
fn main() -> Result<()> {
    let mut args = env::args();
    if args.len() != 2 {
        return Err(anyhow!("Usage: ./tlsv <client_ip>"));
    }

    args.next();
    let client_ip = args.next().unwrap();
    let Ok(client_ip) = client_ip.parse::<IpAddr>() else {
        return Err(anyhow!(
            "Provided client_ip could not be parsed as an IP address."
        ));
    };

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let interfaces = datalink::interfaces();
    let interface = interfaces
        .iter()
        .find(|e| e.is_up() && !e.is_loopback() && !e.ips.is_empty())
        .expect("Could not find interface to bind to.");

    match client_ip {
        IpAddr::V4(client_ip) => process_ipv4(interface, client_ip),
        IpAddr::V6(client_ip) => process_ipv6(interface, client_ip),
    }
    Ok(())
}
