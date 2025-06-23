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

const IPV6_HEADER_LEN: usize = 40;
const NEW_PACKET_TTL: u8 = 64;

macro_rules! extract {
    ($e:expr, $p:path) => {
        match $e {
            $p(value) => Some(value),
            _ => None,
        }
    };
}

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

fn process_ip(interface: &NetworkInterface, client_ip: IpAddr, is_ipv4: bool) -> ! {
    let ethertype = if is_ipv4 {
        0x0800 as u16
    } else {
        0x08DD as u16
    };

    let config = Config {
        channel_type: ChannelType::Layer3(ethertype),
        ..Default::default()
    };
    let (mut tx, mut rx) = match datalink::channel(&interface, config) {
        Ok(Ethernet(tx, rx)) => (tx, rx),
        Ok(_) => panic!("Expected Ethernet channel but found another channel type."),
        Err(e) => panic!("Error occured creating datalink channel: {}", e),
    };

    let mut middlebox = TlsvMiddlebox::new();

    loop {
        match rx.next() {
            Ok(packet) => {
                let src_ip;
                let dest_ip;
                let next_protocol;
                let payload = if is_ipv4 {
                    let ip_packet = Ipv4Packet::new(packet).unwrap();
                    src_ip = IpAddr::V4(ip_packet.get_source());
                    dest_ip = IpAddr::V4(ip_packet.get_destination());
                    next_protocol = ip_packet.get_next_level_protocol();
                    &packet[(ip_packet.get_header_length() * 4) as usize
                        ..ip_packet.get_total_length() as usize]
                } else {
                    let ip_packet = Ipv6Packet::new(packet).unwrap();
                    src_ip = IpAddr::V6(ip_packet.get_source());
                    dest_ip = IpAddr::V6(ip_packet.get_destination());
                    next_protocol = ip_packet.get_next_header();
                    &packet
                        [IPV6_HEADER_LEN..IPV6_HEADER_LEN + ip_packet.get_payload_length() as usize]
                };

                if matches!(next_protocol, IpNextHeaderProtocols::Tcp) {
                    let tcp_packet = TcpPacket::new(payload).unwrap();
                    let flow = TcpFlow {
                        source_ip: src_ip,
                        dest_ip: dest_ip,
                        source_port: tcp_packet.get_source(),
                        dest_port: tcp_packet.get_destination(),
                    };
                    let result = if client_ip == src_ip {
                        Some(middlebox.process_outgoing(&tcp_packet, flow))
                    } else if client_ip == dest_ip {
                        Some(middlebox.process_incoming(&tcp_packet, flow.reverse()))
                    } else {
                        None
                    };

                    if let Some(TlsvResult::Invalid {
                        forward_packet,
                        return_packet,
                    }) = result
                    {
                        if is_ipv4 {
                            tx.build_and_send(
                                1,
                                Ipv4Packet::minimum_packet_size() + forward_packet.packet().len(),
                                &mut |buf| {
                                    prepare_ipv4_packet(
                                        buf,
                                        extract!(src_ip, IpAddr::V4).unwrap(),
                                        extract!(dest_ip, IpAddr::V4).unwrap(),
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
                                        extract!(dest_ip, IpAddr::V4).unwrap(),
                                        extract!(src_ip, IpAddr::V4).unwrap(),
                                        &return_packet,
                                    );
                                },
                            )
                            .expect("No buffer left")
                            .expect("Failed to send packet");
                        } else {
                            tx.build_and_send(
                                1,
                                Ipv6Packet::minimum_packet_size() + forward_packet.packet().len(),
                                &mut |buf| {
                                    prepare_ipv6_packet(
                                        buf,
                                        extract!(src_ip, IpAddr::V6).unwrap(),
                                        extract!(dest_ip, IpAddr::V6).unwrap(),
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
                                        extract!(dest_ip, IpAddr::V6).unwrap(),
                                        extract!(src_ip, IpAddr::V6).unwrap(),
                                        &return_packet,
                                    );
                                },
                            )
                            .expect("No buffer left")
                            .expect("Failed to send packet");
                        }
                        continue;
                    }
                }
                tx.send_to(packet, None);
            }
            Err(e) => {
                panic!("Error occured while reading packet: {}", e);
            }
        }
    }
}

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

    process_ip(interface, client_ip, true);
}
