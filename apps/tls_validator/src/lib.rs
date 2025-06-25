mod protocol;
mod tcp_buffer;

use anyhow::Result;
use colored::Colorize;
use core::panic;
use pnet::packet::{
    Packet,
    tcp::{MutableTcpPacket, TcpFlags, TcpPacket},
};
use rustls::{
    ProtocolVersion, RootCertStore,
    client::{WebPkiServerVerifier, danger::ServerCertVerifier},
    internal::msgs::handshake::{
        ClientExtension, HandshakePayload, ServerExtension, ServerNamePayload,
    },
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use std::{
    collections::HashMap,
    mem,
    net::IpAddr,
    sync::{Arc, LazyLock},
    time::Instant,
};
use tcp_buffer::TcpBuffer;
use tracing::{debug, info, warn};

// we need to detect whether sequence numbers are wrapping or out of order
// instead of monitoring the flow's advertised window, it's simpler to
// use the maximum possible window with the TCP window scale extension
const TCP_MAX_WINDOW: u32 = (2 ^ 16 - 1) * 2 ^ 14;

#[derive(Debug)]
pub enum TlsvResult<'a> {
    Cleared,
    Invalid {
        forward_packet: TcpPacket<'a>,
        return_packet: TcpPacket<'a>,
    },
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub struct TcpFlow {
    pub source_ip: IpAddr,
    pub dest_ip: IpAddr,
    pub source_port: u16,
    pub dest_port: u16,
}

impl TcpFlow {
    pub fn reverse(self) -> TcpFlow {
        TcpFlow {
            source_ip: self.dest_ip,
            dest_ip: self.source_ip,
            source_port: self.dest_port,
            dest_port: self.source_port,
        }
    }
}

#[derive(Debug)]
struct ServerBuffer(TcpBuffer, ServerName<'static>);

#[derive(Debug)]
enum Status {
    PeerInitiated, // external host initiated this TCP connection, doesn't need validation
    Cleared,       // Certs have validated or the flow should otherwise be allowed to continue
    Bad,           // TCP connection should be reset since server certs are bad
    WaitForClientHello(TcpBuffer), // User initiated TCP connection, we are sniffing for ClientHello
    WaitForServerHello(ServerBuffer), // Heard ClientHello and we are now listening for ServerHello
    WaitForCerts(ServerBuffer), // Heard ServerHello and we are now listening for Server Certificates
}

#[derive(Debug)]
struct FlowState {
    last_seen: Instant,
    client_expected_seqno: Option<u32>,
    server_expected_seqno: Option<u32>,
    status: Status,
}

fn process_outgoing_tcp(packet: &TcpPacket, state: &mut FlowState, dest_ip: IpAddr) -> bool {
    debug!("Flow status: {:?}", state.status);

    if let Status::WaitForClientHello(buffer) = &mut state.status {
        buffer.add_packet_data(&packet);

        let Ok(handshake) = protocol::read_handshake_msg(&buffer) else {
            // non-TLS flows are always valid
            debug!("Failed to read ClientHello TLS frame. Probably not a TLS flow?");
            state.status = Status::Cleared;
            return true;
        };
        if let Some(handshake) = handshake {
            let HandshakePayload::ClientHello(client_opts) = handshake.payload else {
                // first TLS record should always be hello...
                debug!("Initial TLS frame not parsed as ClientHello. Probably not a TLS flow?");
                state.status = Status::Cleared;
                return true;
            };

            // Note: don't check if client's advertised TLS version is 1.2
            // instead, we will check TLS version inside ServerHello

            // Sanity Check:
            if state.client_expected_seqno.is_none() {
                // this should be impossible since server must send back ACKs during TCP
                // handshake, which initializes server_sent_seqno
                warn!("Saw ClientHello but haven't yet heard from server?");
                state.status = Status::Cleared;
                return true;
            }

            let mut server_name = None;
            for ext in client_opts.extensions {
                if let ClientExtension::ServerName(name) = ext {
                    match name {
                        ServerNamePayload::SingleDnsName(dns_name) => {
                            server_name = Some(ServerName::DnsName(dns_name));
                        }
                        ServerNamePayload::IpAddress => {
                            server_name = Some(ServerName::IpAddress(dest_ip.into()));
                        }
                        ServerNamePayload::Invalid => {}
                    }
                }
            }
            if server_name.is_none() {
                // We do not support TLS connection without SNI
                // realistically, all modern clients use SNI
                warn!("Blocked TLS connection without SNI!");
                state.status = Status::Bad;
                return false;
            }
            let server_name = server_name.unwrap();
            debug!("Found TLS connection for server name: {:?}", server_name);

            let new_buffer = TcpBuffer::new(
                state.client_expected_seqno.unwrap(),
                2 * (protocol::TLS_HEADER_LENGTH + protocol::TLS_MAX_RECORD_LENGTH) as usize,
            );
            state.status = Status::WaitForServerHello(ServerBuffer(new_buffer, server_name));
            debug!("Transitioning to WaitForServerHello");
        }
    }

    return !matches!(state.status, Status::Bad);
}

fn process_incoming_tcp(packet: &TcpPacket, state: &mut FlowState) -> bool {
    debug!("Flow status: {:?}", state.status);

    match &mut state.status {
        Status::WaitForServerHello(ServerBuffer(buffer, _)) => {
            buffer.add_packet_data(&packet);

            let Ok(handshake) = protocol::read_handshake_msg(buffer) else {
                warn!("Failed to read ServerHello TLS frame: {:?}", state);
                state.status = Status::Cleared;
                return true;
            };
            if let Some(handshake) = handshake {
                let HandshakePayload::ServerHello(server_opts) = handshake.payload else {
                    warn!("Second TLS frame not parsed as ServerHello: {:?}", state);
                    state.status = Status::Cleared;
                    return true;
                };

                // For TLSv1.2 and previous, the TLS version in the TLS frame is accurate
                if !handshake.is_tls12 {
                    debug!("ServerHello frame is not TLSv1.2, probably using an older version.");
                    state.status = Status::Cleared;
                    return true;
                }

                // For backwards compatibility, the frame verion is TLSv1.2 when using TLSv1.3
                // to detect the actual version, we must check Supported Versions extension
                for ext in server_opts.extensions {
                    if let ServerExtension::SupportedVersions(version) = ext {
                        if !matches!(version, ProtocolVersion::TLSv1_2) {
                            debug!(
                                "ServerHello supported version is not TLSv1.2, probably using v1.3"
                            );
                            state.status = Status::Cleared;
                            return true;
                        }
                        break;
                    }
                }

                buffer.drain(handshake.total_len);
                // must temporarily replace status with Status::Cleared to take ownership of it
                let status = mem::replace(&mut state.status, Status::Cleared);
                match status {
                    Status::WaitForServerHello(data) => {
                        state.status = Status::WaitForCerts(data);
                        debug!("Transitioning to WaitForCerts!");
                    }
                    _ => {
                        panic!("Impossible! Status must be Status::WaitForServerHello")
                    }
                }
            }
        }
        Status::WaitForCerts(ServerBuffer(buffer, name)) => {
            buffer.add_packet_data(&packet);

            let Ok(handshake) = protocol::read_handshake_msg(buffer) else {
                debug!(
                    "Failed to read Certificate TLS frame. Maybe renegotiating old TLS connection?"
                );
                state.status = Status::Cleared;
                return true;
            };
            if let Some(handshake) = handshake {
                let HandshakePayload::Certificate(certs) = handshake.payload else {
                    warn!("Third TLS frame not parsed as Certificates: {:?}", state);
                    state.status = Status::Cleared;
                    return true;
                };
                let certs = certs.0;

                if certs.len() == 0 || !validate_certs(&certs, name) {
                    info!("Certs are invalid for {:?}!", name);
                    state.status = Status::Bad;
                } else {
                    info!("Certs are valid and flow is cleared for {:?}!", name);
                    state.status = Status::Cleared;
                }
            }
        }
        _ => {}
    }

    return !matches!(state.status, Status::Bad);
}

pub struct TlsvMiddlebox {
    flows: HashMap<TcpFlow, FlowState>,
}

impl TlsvMiddlebox {
    pub fn new() -> TlsvMiddlebox {
        TlsvMiddlebox {
            flows: HashMap::new(),
        }
    }

    fn update_flow_state(
        &mut self,
        packet: &TcpPacket,
        flow: TcpFlow,
        is_outgoing: bool,
    ) -> Result<&mut FlowState, ()> {
        if is_outgoing {
            let fmt = format!("{:?}", flow).bright_blue();
            debug!("Outgoing: seqno={}, {}", packet.get_sequence(), fmt);
        } else {
            let fmt = format!("{:?}", flow).bright_red();
            debug!("Incoming: seqno={}, {}", packet.get_sequence(), fmt);
        }

        let timestamp = Instant::now();
        let state;
        if packet.get_flags() & TcpFlags::SYN != 0 {
            if !self.flows.contains_key(&flow) {
                debug!("Creating new flow!");
            }

            let initial_status = if is_outgoing {
                Status::WaitForClientHello(TcpBuffer::new(
                    packet.get_sequence() + 1,
                    2 * (protocol::TLS_HEADER_LENGTH + protocol::TLS_MAX_RECORD_LENGTH) as usize,
                ))
            } else {
                Status::PeerInitiated
            };

            state = self.flows.entry(flow).or_insert(FlowState {
                last_seen: timestamp,
                client_expected_seqno: None,
                server_expected_seqno: None,
                status: initial_status,
            });
        } else {
            if let Some(value) = self.flows.get_mut(&flow) {
                state = value;
            } else {
                // never saw the TCP handshake for this flow?
                // should be impossible
                return Err(());
            }
        }

        state.last_seen = timestamp;

        let seqno_target = if is_outgoing {
            &mut state.server_expected_seqno
        } else {
            &mut state.client_expected_seqno
        };

        let mut next_seqno = packet
            .get_sequence()
            .wrapping_add(packet.payload().len() as u32);
        if packet.get_flags() & TcpFlags::SYN != 0 {
            next_seqno += 1;
        }
        if seqno_target.is_none()
            || seqno_target.unwrap() < next_seqno
            || (next_seqno + (u32::MAX - seqno_target.unwrap())) < TCP_MAX_WINDOW
        {
            *seqno_target = Some(next_seqno);
        }

        Ok(state)
    }

    fn validate_flow(
        &mut self,
        packet: &TcpPacket,
        flow: TcpFlow,
        is_outgoing: bool,
    ) -> TlsvResult<'static> {
        let src_ip = flow.source_ip.clone();
        let dest_ip = flow.dest_ip.clone();
        if let Ok(flow_state) = self.update_flow_state(packet, flow, is_outgoing) {
            let valid;
            let rst_seqno;
            if is_outgoing {
                valid = process_outgoing_tcp(packet, flow_state, dest_ip.clone());
                rst_seqno = flow_state.client_expected_seqno.unwrap_or(0);
            } else {
                valid = process_incoming_tcp(packet, flow_state);
                rst_seqno = flow_state.server_expected_seqno.unwrap_or(0);
            };

            if !valid {
                let packet_rst_data = packet.packet().to_owned();
                let mut packet_rst = MutableTcpPacket::owned(packet_rst_data).unwrap();
                packet_rst.set_flags(packet_rst.get_flags() | TcpFlags::RST);

                let mut return_packet = protocol::generate_return_rst(packet, rst_seqno);

                if is_outgoing {
                    protocol::set_tcp_checksum(&mut packet_rst, src_ip, dest_ip);
                    protocol::set_tcp_checksum(&mut return_packet, src_ip, dest_ip);
                } else {
                    protocol::set_tcp_checksum(&mut packet_rst, dest_ip, src_ip);
                    protocol::set_tcp_checksum(&mut return_packet, dest_ip, src_ip);
                }

                return TlsvResult::Invalid {
                    forward_packet: packet_rst.consume_to_immutable(),
                    return_packet: return_packet.consume_to_immutable(),
                };
            }
        } else {
            warn!("Un-recognized flow for IPv4/TCP packet!");
        }
        TlsvResult::Cleared
    }

    pub fn process_outgoing(
        &mut self,
        tcp_packet: &TcpPacket,
        flow: TcpFlow,
    ) -> TlsvResult<'static> {
        self.validate_flow(tcp_packet, flow, true)
    }

    pub fn process_incoming(
        &mut self,
        tcp_packet: &TcpPacket,
        flow: TcpFlow,
    ) -> TlsvResult<'static> {
        self.validate_flow(tcp_packet, flow, false)
    }
}

static VALIDATOR: LazyLock<Arc<WebPkiServerVerifier>> = LazyLock::new(|| {
    let root_store = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    WebPkiServerVerifier::builder(root_store.into())
        .build()
        .unwrap()
});

fn validate_certs(certs: &[CertificateDer], name: &ServerName) -> bool {
    if certs.len() == 0 {
        return false;
    }

    let verified =
        VALIDATOR.verify_server_cert(&certs[0], &certs[1..], &name, &[], UnixTime::now());
    return verified.is_ok();
}
