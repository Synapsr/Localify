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

/// The addresses a query for a local name resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Answer {
    ipv4: Ipv4Addr,
    /// Only this machine reaches Localify on IPv6, through `::1`. For another
    /// device that address would be itself, so it gets no IPv6 answer at all.
    ipv6: Option<Ipv6Addr>,
}

/// Resolves the configured local suffix to the best host address for the caller.
#[derive(Clone)]
pub struct IpResolver {
    lan_ip: LanIpProvider,
    lan_access: bool,
}

impl IpResolver {
    pub fn new(lan_access: bool) -> Self {
        Self {
            lan_ip: Arc::new(get_lan_ip),
            lan_access,
        }
    }

    #[cfg(test)]
    fn with_lan_ip(lan_ip: Ipv4Addr, lan_access: bool) -> Self {
        Self {
            lan_ip: Arc::new(move || lan_ip),
            lan_access,
        }
    }

    /// Local callers should never depend on the current network interface, and
    /// no caller gets the LAN address while the proxy does not listen on it.
    fn resolve(&self, client_ip: IpAddr) -> Answer {
        if client_ip.is_loopback() {
            return Answer {
                ipv4: Ipv4Addr::LOCALHOST,
                ipv6: Some(Ipv6Addr::LOCALHOST),
            };
        }
        Answer {
            ipv4: if self.lan_access {
                (self.lan_ip)()
            } else {
                Ipv4Addr::LOCALHOST
            },
            ipv6: None,
        }
    }

    fn current_lan_ip(&self) -> Ipv4Addr {
        (self.lan_ip)()
    }
}

pub struct DnsServer {
    port: u16,
    lan_access: bool,
    ttl: u32,
    ip_resolver: Arc<IpResolver>,
}

impl DnsServer {
    pub fn new(port: u16, lan_access: bool) -> Self {
        Self {
            port,
            lan_access,
            ttl: 1,
            ip_resolver: Arc::new(IpResolver::new(lan_access)),
        }
    }

    pub async fn run(&self) -> Result<()> {
        let (ipv4_ip, ipv6_ip) = if self.lan_access {
            (Ipv4Addr::UNSPECIFIED, Ipv6Addr::UNSPECIFIED)
        } else {
            (Ipv4Addr::LOCALHOST, Ipv6Addr::LOCALHOST)
        };
        let ipv4_addr = SocketAddr::from((ipv4_ip, self.port));
        let ipv6_addr = SocketAddr::from((ipv6_ip, self.port));

        // Bind UDP sockets
        let udp_v4 = UdpSocket::bind(ipv4_addr).await?;
        let tcp_v4 = TcpListener::bind(ipv4_addr).await?;

        // IPv6 binds may fail on Linux where the IPv4 dual-stack socket already
        // covers both protocols. This is fine — we just skip IPv6-specific listeners.
        let udp_v6 = UdpSocket::bind(ipv6_addr).await.ok();
        let tcp_v6 = TcpListener::bind(ipv6_addr).await.ok();

        if udp_v6.is_some() {
            info!(
                ipv4 = %ipv4_addr,
                ipv6 = %ipv6_addr,
                current_lan_ip = %self.ip_resolver.current_lan_ip(),
                "DNS server listening"
            );
        } else {
            info!(
                ipv4 = %ipv4_addr,
                current_lan_ip = %self.ip_resolver.current_lan_ip(),
                "DNS server listening (IPv6 unavailable, using IPv4 dual-stack)"
            );
        }

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
        let response = handle_query(&buf[..len], ttl, resolver.resolve(addr.ip()));
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

    let response = handle_query(&query_buf, ttl, resolver.resolve(addr.ip()));

    // Send response with length prefix
    let resp_len = (response.len() as u16).to_be_bytes();
    stream.write_all(&resp_len).await?;
    stream.write_all(&response).await?;

    Ok(())
}

fn handle_query(query: &[u8], ttl: u32, answer: Answer) -> Vec<u8> {
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
        response = %answer.ipv4,
        "DNS query"
    );

    // Build response based on query type
    match question.qtype {
        QTYPE::TYPE(TYPE::A) => build_a_response(&packet, question, ttl, answer.ipv4),
        QTYPE::TYPE(TYPE::AAAA) => match answer.ipv6 {
            Some(ipv6) => build_aaaa_response(&packet, question, ttl, ipv6),
            None => build_empty_response(&packet),
        },
        QTYPE::ANY => build_any_response(&packet, question, ttl, answer),
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

fn build_aaaa_response(packet: &Packet, question: &Question, ttl: u32, ip: Ipv6Addr) -> Vec<u8> {
    let mut response = Packet::new_reply(packet.id());
    response.set_flags(
        PacketFlag::RESPONSE | PacketFlag::AUTHORITATIVE_ANSWER | PacketFlag::RECURSION_DESIRED,
    );
    *response.rcode_mut() = RCODE::NoError;

    // Add the question
    response.questions.push(question.clone());

    // Add AAAA record
    let name_str = question.qname.to_string();
    let name = Name::new_unchecked(&name_str);
    let aaaa_record = AAAA::from(ip);
    let record = ResourceRecord::new(name, CLASS::IN, ttl, RData::AAAA(aaaa_record));
    response.answers.push(record);

    response.build_bytes_vec().unwrap_or_default()
}

fn build_any_response(packet: &Packet, question: &Question, ttl: u32, answer: Answer) -> Vec<u8> {
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
    let a_record = A::from(answer.ipv4);
    response.answers.push(ResourceRecord::new(
        name_a,
        CLASS::IN,
        ttl,
        RData::A(a_record),
    ));

    // Add AAAA record
    if let Some(ipv6) = answer.ipv6 {
        let name_aaaa = Name::new_unchecked(&name_str);
        let aaaa_record = AAAA::from(ipv6);
        response.answers.push(ResourceRecord::new(
            name_aaaa,
            CLASS::IN,
            ttl,
            RData::AAAA(aaaa_record),
        ));
    }

    response.build_bytes_vec().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 100);
    const REMOTE: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));

    fn test_resolver() -> IpResolver {
        IpResolver::with_lan_ip(TEST_IP, true)
    }

    fn answer() -> Answer {
        Answer {
            ipv4: TEST_IP,
            ipv6: Some(Ipv6Addr::LOCALHOST),
        }
    }

    fn query(name: &str, record: TYPE) -> Vec<u8> {
        let mut packet = Packet::new_query(1234);
        let question = Question::new(
            Name::new_unchecked(name),
            record.into(),
            CLASS::IN.into(),
            false,
        );
        packet.questions.push(question);
        packet.build_bytes_vec().unwrap()
    }

    #[test]
    fn test_ip_resolver_returns_loopback_for_local_clients() {
        let resolver = test_resolver();
        assert_eq!(
            resolver.resolve(IpAddr::V4(Ipv4Addr::LOCALHOST)).ipv4,
            Ipv4Addr::LOCALHOST
        );
        assert_eq!(
            resolver.resolve(IpAddr::V6(Ipv6Addr::LOCALHOST)).ipv4,
            Ipv4Addr::LOCALHOST
        );
        assert_eq!(
            resolver.resolve(IpAddr::V4(Ipv4Addr::LOCALHOST)).ipv6,
            Some(Ipv6Addr::LOCALHOST)
        );
    }

    #[test]
    fn test_ip_resolver_returns_lan_ip_for_remote_clients() {
        let resolver = test_resolver();
        assert_eq!(resolver.resolve(REMOTE).ipv4, TEST_IP);
    }

    #[test]
    fn remote_clients_get_no_ipv6_address_because_loopback_would_be_themselves() {
        assert_eq!(test_resolver().resolve(REMOTE).ipv6, None);

        let response = handle_query(
            &query("test.localify", TYPE::AAAA),
            1,
            test_resolver().resolve(REMOTE),
        );
        let parsed = Packet::parse(&response).unwrap();
        assert_eq!(parsed.rcode(), RCODE::NoError);
        assert_eq!(parsed.answers.len(), 0);
    }

    #[test]
    fn no_client_learns_the_lan_address_without_the_opt_in() {
        let resolver = IpResolver::with_lan_ip(TEST_IP, false);

        assert_eq!(resolver.resolve(REMOTE).ipv4, Ipv4Addr::LOCALHOST);
        assert_eq!(resolver.resolve(REMOTE).ipv6, None);
    }

    #[test]
    fn test_query_handler_localify_domain() {
        // Build a simple A query for test.localify
        let mut packet = Packet::new_query(1234);
        let name = Name::new_unchecked("test.localify");
        let question = Question::new(name, TYPE::A.into(), CLASS::IN.into(), false);
        packet.questions.push(question);

        let query = packet.build_bytes_vec().unwrap();
        let response = handle_query(&query, 1, answer());

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
        let response = handle_query(&query, 1, answer());

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
        let response = handle_query(&query, 1, answer());

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
        let response = handle_query(&query, 1, answer());

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
        let response = handle_query(
            &query,
            1,
            Answer {
                ipv4: custom_ip,
                ipv6: None,
            },
        );

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
