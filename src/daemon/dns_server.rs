use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use anyhow::Result;
use simple_dns::rdata::{A, AAAA, RData};
use simple_dns::{CLASS, Name, Packet, PacketFlag, QTYPE, Question, RCODE, ResourceRecord, TYPE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tracing::{debug, info};

use crate::branding::BRANDING;
use crate::infrastructure::network::get_lan_ip;

type LanIpProvider = Arc<dyn Fn() -> Ipv4Addr + Send + Sync>;

/// Resolves the configured local suffix to the best host address for the caller.
#[derive(Clone)]
pub struct IpResolver {
    lan_ip: LanIpProvider,
}

impl IpResolver {
    pub fn new() -> Self {
        Self {
            lan_ip: Arc::new(get_lan_ip),
        }
    }

    #[cfg(test)]
    fn with_lan_ip(lan_ip: Ipv4Addr) -> Self {
        Self {
            lan_ip: Arc::new(move || lan_ip),
        }
    }

    /// Local callers should never depend on the current network interface.
    pub fn resolve(&self, client_ip: IpAddr) -> Ipv4Addr {
        if client_ip.is_loopback() {
            Ipv4Addr::LOCALHOST
        } else {
            (self.lan_ip)()
        }
    }

    fn current_lan_ip(&self) -> Ipv4Addr {
        (self.lan_ip)()
    }
}

/// Sockets the DNS server answers on, bound before the daemon drops root.
pub struct DnsSockets {
    udp_v4: std::net::UdpSocket,
    tcp_v4: std::net::TcpListener,
    udp_v6: Option<std::net::UdpSocket>,
    tcp_v6: Option<std::net::TcpListener>,
}

impl DnsSockets {
    pub fn bind(port: u16) -> Result<Self> {
        // Bind to all interfaces so Docker containers can reach us directly
        let ipv4_addr = SocketAddr::from(([0, 0, 0, 0], port));
        let ipv6_addr = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port));

        let udp_v4 = std::net::UdpSocket::bind(ipv4_addr)?;
        let tcp_v4 = std::net::TcpListener::bind(ipv4_addr)?;
        udp_v4.set_nonblocking(true)?;
        tcp_v4.set_nonblocking(true)?;

        // IPv6 binds may fail on Linux where the IPv4 dual-stack socket already
        // covers both protocols. This is fine — we just skip IPv6-specific listeners.
        let udp_v6 = std::net::UdpSocket::bind(ipv6_addr)
            .and_then(|socket| socket.set_nonblocking(true).map(|()| socket))
            .ok();
        let tcp_v6 = std::net::TcpListener::bind(ipv6_addr)
            .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
            .ok();

        Ok(Self {
            udp_v4,
            tcp_v4,
            udp_v6,
            tcp_v6,
        })
    }
}

pub struct DnsServer {
    sockets: DnsSockets,
    ttl: u32,
    ip_resolver: Arc<IpResolver>,
}

impl DnsServer {
    pub fn new(sockets: DnsSockets) -> Self {
        Self {
            sockets,
            ttl: 1,
            ip_resolver: Arc::new(IpResolver::new()),
        }
    }

    pub async fn run(self) -> Result<()> {
        let udp_v4 = UdpSocket::from_std(self.sockets.udp_v4)?;
        let tcp_v4 = TcpListener::from_std(self.sockets.tcp_v4)?;
        let udp_v6 = self.sockets.udp_v6.map(UdpSocket::from_std).transpose()?;
        let tcp_v6 = self.sockets.tcp_v6.map(TcpListener::from_std).transpose()?;

        info!(
            ipv4 = %udp_v4.local_addr()?,
            ipv6_listener = udp_v6.is_some(),
            current_lan_ip = %self.ip_resolver.current_lan_ip(),
            "DNS server listening"
        );

        let ttl = self.ttl;
        let resolver = self.ip_resolver.clone();

        tokio::select! {
            r = serve_udp(udp_v4, ttl, resolver.clone()) => r,
            r = serve_tcp(tcp_v4, ttl, resolver.clone()) => r,
            r = async {
                if let Some(s) = udp_v6 { serve_udp(s, ttl, resolver.clone()).await } else { std::future::pending().await }
            } => r,
            r = async {
                if let Some(s) = tcp_v6 { serve_tcp(s, ttl, resolver.clone()).await } else { std::future::pending().await }
            } => r,
        }
    }
}

async fn serve_udp(socket: UdpSocket, ttl: u32, resolver: Arc<IpResolver>) -> Result<()> {
    let mut buf = [0u8; 512]; // Standard DNS UDP size

    loop {
        let (len, addr) = socket.recv_from(&mut buf).await?;
        let response_ip = resolver.resolve(addr.ip());
        let response = handle_query(&buf[..len], ttl, response_ip);
        let _ = socket.send_to(&response, addr).await;
    }
}

async fn serve_tcp(listener: TcpListener, ttl: u32, resolver: Arc<IpResolver>) -> Result<()> {
    loop {
        let (stream, addr) = listener.accept().await?;
        tokio::spawn(handle_tcp_connection(stream, ttl, resolver.clone(), addr));
    }
}

async fn handle_tcp_connection(
    mut stream: TcpStream,
    ttl: u32,
    resolver: Arc<IpResolver>,
    addr: SocketAddr,
) -> Result<()> {
    // TCP DNS uses 2-byte length prefix
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_be_bytes(len_buf) as usize;

    let mut query_buf = vec![0u8; len];
    stream.read_exact(&mut query_buf).await?;

    let response_ip = resolver.resolve(addr.ip());
    let response = handle_query(&query_buf, ttl, response_ip);

    // Send response with length prefix
    let resp_len = (response.len() as u16).to_be_bytes();
    stream.write_all(&resp_len).await?;
    stream.write_all(&response).await?;

    Ok(())
}

fn handle_query(query: &[u8], ttl: u32, response_ip: Ipv4Addr) -> Vec<u8> {
    // Parse incoming query
    let packet = match Packet::parse(query) {
        Ok(p) => p,
        Err(_) => return build_format_error(query),
    };

    let question = match packet.questions.first() {
        Some(q) => q,
        None => return build_format_error(query),
    };

    let domain = question.qname.to_string().to_lowercase();

    if !domain
        .trim_end_matches('.')
        .ends_with(BRANDING.domain_suffix())
    {
        debug!(
            domain = %domain,
            suffix = BRANDING.domain_suffix(),
            "DNS refused (unexpected suffix)"
        );
        return build_refused_response(&packet);
    }

    info!(
        domain = %domain,
        qtype = ?question.qtype,
        response = %response_ip,
        "DNS query"
    );

    // Build response based on query type
    match question.qtype {
        QTYPE::TYPE(TYPE::A) => build_a_response(&packet, question, ttl, response_ip),
        QTYPE::TYPE(TYPE::AAAA) => build_aaaa_response(&packet, question, ttl),
        QTYPE::ANY => build_any_response(&packet, question, ttl, response_ip),
        _ => build_empty_response(&packet),
    }
}

fn build_format_error(query: &[u8]) -> Vec<u8> {
    // Try to extract transaction ID from query
    let id = if query.len() >= 2 {
        u16::from_be_bytes([query[0], query[1]])
    } else {
        0
    };

    let mut response = Packet::new_reply(id);
    response.set_flags(PacketFlag::RESPONSE | PacketFlag::RECURSION_DESIRED);
    *response.rcode_mut() = RCODE::FormatError;

    response.build_bytes_vec().unwrap_or_default()
}

fn build_refused_response(packet: &Packet) -> Vec<u8> {
    let mut response = Packet::new_reply(packet.id());
    response.set_flags(PacketFlag::RESPONSE | PacketFlag::RECURSION_DESIRED);
    *response.rcode_mut() = RCODE::Refused;

    response.build_bytes_vec().unwrap_or_default()
}

fn build_empty_response(packet: &Packet) -> Vec<u8> {
    let mut response = Packet::new_reply(packet.id());
    response.set_flags(
        PacketFlag::RESPONSE | PacketFlag::AUTHORITATIVE_ANSWER | PacketFlag::RECURSION_DESIRED,
    );
    *response.rcode_mut() = RCODE::NoError;

    // Clone questions into the response
    for q in &packet.questions {
        response.questions.push(q.clone());
    }

    response.build_bytes_vec().unwrap_or_default()
}

fn build_a_response(packet: &Packet, question: &Question, ttl: u32, ip: Ipv4Addr) -> Vec<u8> {
    let mut response = Packet::new_reply(packet.id());
    response.set_flags(
        PacketFlag::RESPONSE | PacketFlag::AUTHORITATIVE_ANSWER | PacketFlag::RECURSION_DESIRED,
    );
    *response.rcode_mut() = RCODE::NoError;

    // Add the question
    response.questions.push(question.clone());

    // Add A record
    let name_str = question.qname.to_string();
    let name = Name::new_unchecked(&name_str);
    let a_record = A::from(ip);
    let record = ResourceRecord::new(name, CLASS::IN, ttl, RData::A(a_record));
    response.answers.push(record);

    response.build_bytes_vec().unwrap_or_default()
}

fn build_aaaa_response(packet: &Packet, question: &Question, ttl: u32) -> Vec<u8> {
    let mut response = Packet::new_reply(packet.id());
    response.set_flags(
        PacketFlag::RESPONSE | PacketFlag::AUTHORITATIVE_ANSWER | PacketFlag::RECURSION_DESIRED,
    );
    *response.rcode_mut() = RCODE::NoError;

    // Add the question
    response.questions.push(question.clone());

    // Add AAAA record: ::1
    let name_str = question.qname.to_string();
    let name = Name::new_unchecked(&name_str);
    let aaaa_record = AAAA::from(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1));
    let record = ResourceRecord::new(name, CLASS::IN, ttl, RData::AAAA(aaaa_record));
    response.answers.push(record);

    response.build_bytes_vec().unwrap_or_default()
}

fn build_any_response(packet: &Packet, question: &Question, ttl: u32, ip: Ipv4Addr) -> Vec<u8> {
    let mut response = Packet::new_reply(packet.id());
    response.set_flags(
        PacketFlag::RESPONSE | PacketFlag::AUTHORITATIVE_ANSWER | PacketFlag::RECURSION_DESIRED,
    );
    *response.rcode_mut() = RCODE::NoError;

    // Add the question
    response.questions.push(question.clone());

    // Add A record
    let name_str = question.qname.to_string();
    let name_a = Name::new_unchecked(&name_str);
    let a_record = A::from(ip);
    response.answers.push(ResourceRecord::new(
        name_a,
        CLASS::IN,
        ttl,
        RData::A(a_record),
    ));

    // Add AAAA record
    let name_aaaa = Name::new_unchecked(&name_str);
    let aaaa_record = AAAA::from(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1));
    response.answers.push(ResourceRecord::new(
        name_aaaa,
        CLASS::IN,
        ttl,
        RData::AAAA(aaaa_record),
    ));

    response.build_bytes_vec().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 100);

    fn test_resolver() -> IpResolver {
        IpResolver::with_lan_ip(TEST_IP)
    }

    #[test]
    fn test_ip_resolver_returns_loopback_for_local_clients() {
        let resolver = test_resolver();
        assert_eq!(
            resolver.resolve(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            Ipv4Addr::LOCALHOST
        );
        assert_eq!(
            resolver.resolve(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            Ipv4Addr::LOCALHOST
        );
    }

    #[test]
    fn test_ip_resolver_returns_lan_ip_for_remote_clients() {
        let resolver = test_resolver();
        assert_eq!(
            resolver.resolve(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))),
            TEST_IP
        );
    }

    #[test]
    fn test_query_handler_localify_domain() {
        // Build a simple A query for test.localify
        let mut packet = Packet::new_query(1234);
        let name = Name::new_unchecked("test.localify");
        let question = Question::new(name, TYPE::A.into(), CLASS::IN.into(), false);
        packet.questions.push(question);

        let query = packet.build_bytes_vec().unwrap();
        let response = handle_query(&query, 1, TEST_IP);

        let parsed = Packet::parse(&response).unwrap();
        assert_eq!(parsed.rcode(), RCODE::NoError);
        assert_eq!(parsed.answers.len(), 1);
    }

    #[test]
    fn test_query_handler_non_localify_domain() {
        // Build a simple A query for google.com
        let mut packet = Packet::new_query(1234);
        let name = Name::new_unchecked("google.com");
        let question = Question::new(name, TYPE::A.into(), CLASS::IN.into(), false);
        packet.questions.push(question);

        let query = packet.build_bytes_vec().unwrap();
        let response = handle_query(&query, 1, TEST_IP);

        let parsed = Packet::parse(&response).unwrap();
        assert_eq!(parsed.rcode(), RCODE::Refused);
        assert_eq!(parsed.answers.len(), 0);
    }

    #[test]
    fn test_query_handler_aaaa() {
        let mut packet = Packet::new_query(5678);
        let name = Name::new_unchecked("test.localify");
        let question = Question::new(name, TYPE::AAAA.into(), CLASS::IN.into(), false);
        packet.questions.push(question);

        let query = packet.build_bytes_vec().unwrap();
        let response = handle_query(&query, 1, TEST_IP);

        let parsed = Packet::parse(&response).unwrap();
        assert_eq!(parsed.rcode(), RCODE::NoError);
        assert_eq!(parsed.answers.len(), 1);
    }

    #[test]
    fn test_query_handler_subdomain() {
        // Test nested subdomains like app.test.localify
        let mut packet = Packet::new_query(9999);
        let name = Name::new_unchecked("app.test.localify");
        let question = Question::new(name, TYPE::A.into(), CLASS::IN.into(), false);
        packet.questions.push(question);

        let query = packet.build_bytes_vec().unwrap();
        let response = handle_query(&query, 1, TEST_IP);

        let parsed = Packet::parse(&response).unwrap();
        assert_eq!(parsed.rcode(), RCODE::NoError);
        assert_eq!(parsed.answers.len(), 1);
    }

    #[test]
    fn test_a_response_uses_configured_ip() {
        let mut packet = Packet::new_query(1111);
        let name = Name::new_unchecked("test.localify");
        let question = Question::new(name, TYPE::A.into(), CLASS::IN.into(), false);
        packet.questions.push(question);

        let query = packet.build_bytes_vec().unwrap();
        let custom_ip = Ipv4Addr::new(10, 0, 0, 50);
        let response = handle_query(&query, 1, custom_ip);

        let parsed = Packet::parse(&response).unwrap();
        assert_eq!(parsed.rcode(), RCODE::NoError);
        assert_eq!(parsed.answers.len(), 1);

        // Verify the IP in the response
        if let RData::A(a_record) = &parsed.answers[0].rdata {
            assert_eq!(*a_record, A::from(custom_ip));
        } else {
            panic!("Expected A record");
        }
    }
}
