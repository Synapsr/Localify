//! Runs a real daemon with a signed-in machine against a fake Localify Cloud
//! and a fake `cloudflared`, then sends public traffic to its ingress the way
//! the tunnel would: HTTP and a WebSocket upgrade to a local upstream.
//! Everything lives in a temporary directory, on free ports, under the dev
//! profile, so an installed Localify is never touched.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use localify::infrastructure::cloudflared;

const SECRET: &str = "6f2a0c4e8b1d3f5a7c9e0b2d4f6a8c1e3b5d7f9a0c2e4b6d8f1a3c5e7b9d0f2a";
const TOKEN: &str = "tunnel-token-for-tests";
const PUBLIC_HOST: &str = "journal-a8f3--web.localify.fyi";
const LOCAL_HOST: &str = "web.journal.localify.test";
const VISITOR: &str = "203.0.113.7";

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Reads one request head, up to the blank line.
fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).unwrap_or(0) == 0 {
            break;
        }
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head).to_string()
}

/// Answers HTTP with the request head it received as the body, and a
/// WebSocket handshake with 101 and then an echo. Every head it sees goes to
/// `heads`.
fn upstream(heads: mpsc::Sender<String>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let heads = heads.clone();
            thread::spawn(move || serve_upstream(stream, heads));
        }
    });
    port
}

fn serve_upstream(mut stream: TcpStream, heads: mpsc::Sender<String>) {
    let head = read_head(&mut stream);
    let _ = heads.send(head.clone());
    if head.to_ascii_lowercase().contains("upgrade: websocket") {
        let _ = stream.write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: accepted\r\n\r\n",
        );
        let mut buffer = [0u8; 1024];
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0 || stream.write_all(&buffer[..read]).is_err() {
                break;
            }
        }
        return;
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{head}",
        head.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// Answers every request with this machine's `ready` runtime: one wildcard
/// URL on the `journal` project's namespace, which no service registers.
fn fake_cloud() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = format!(
        r#"{{"state":"ready","blocked":null,"tunnel":{{"generation":1,"token":"{TOKEN}"}},
            "ingressSecret":"{SECRET}","retryAfter":null,"publicUrls":[{{
            "id":"u1","slug":"journal-a8f3","url":"https://journal-a8f3.localify.fyi",
            "localDomain":"journal.localify.test","wildcard":true,
            "machine":{{"id":"m1","name":"runner"}},"protection":{{"mode":"password","version":1}},
            "state":"serving","createdAt":"2026-11-05T18:00:00Z"}}]}}"#
    );
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            read_head(&mut stream);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    port
}

/// Records how it was started, then waits like `cloudflared`.
const FAKE_CLOUDFLARED: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
printf '%s\n' "$@" > "$dir/args"
[ "$TUNNEL_TOKEN" = "tunnel-token-for-tests" ] && echo yes > "$dir/token"
echo $$ > "$dir/pid"
exec sleep 120
"#;

struct Setup {
    root: tempfile::TempDir,
    ingress_port: u16,
}

impl Setup {
    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn bin(&self) -> PathBuf {
        cloudflared::binary_path(&self.path("data"))
            .parent()
            .unwrap()
            .to_path_buf()
    }
}

fn set_up(upstream_port: u16, cloud_port: u16) -> Setup {
    let root = tempfile::tempdir().unwrap();
    let path = |name: &str| root.path().join(name);
    std::fs::create_dir_all(path("home")).unwrap();
    std::fs::create_dir_all(path("data")).unwrap();
    let config = format!(
        r#"
[daemon]
http_port = {http}
https_port = {https}
dns_port = {dns}
log_level = "info"
lan_access = false

[paths]
data_dir = "{data}"
pid_file = "{root}/localify.pid"
log_file = "{root}/logs/localify.log"
certs_dir = "{root}/certs"
socket_path = "{root}/localify.sock"

[docker]
enabled = false

[projects.journal]
name = "Journal"

[domains."{LOCAL_HOST}"]
pattern = "{LOCAL_HOST}"
project = "journal"

[[domains."{LOCAL_HOST}".routes]]
path = "/"
target = "127.0.0.1:{upstream_port}"
"#,
        http = free_port(),
        https = free_port(),
        dns = free_port(),
        data = path("data").display(),
        root = root.path().display(),
    );
    std::fs::write(path("config.toml"), config).unwrap();
    let credentials = format!(
        r#"{{"apiBase":"http://127.0.0.1:{cloud_port}","token":"lfy_test","login":"teo","machineId":"m1","machineName":"runner"}}"#
    );
    std::fs::write(path("data").join("cloud.json"), credentials).unwrap();
    std::fs::set_permissions(
        path("data").join("cloud.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    let binary = cloudflared::binary_path(&path("data"));
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(&binary, FAKE_CLOUDFLARED).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

    Setup {
        root,
        ingress_port: free_port(),
    }
}

/// Stops the daemon through its socket, and kills it if it does not go.
struct Daemon {
    child: Child,
    socket: PathBuf,
}

impl Daemon {
    fn start(setup: &Setup) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_localify"))
            .arg("--config")
            .arg(setup.path("config.toml"))
            .args(["start", "--foreground"])
            .env("HOME", setup.path("home"))
            .env("LOCALIFY_PROFILE", "dev")
            .env(
                "LOCALIFY_PUBLIC_INGRESS_PORT",
                setup.ingress_port.to_string(),
            )
            .env_remove("LOCALIFY_CLOUD_API")
            .env_remove("LOCALIFY_LOG")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            child,
            socket: setup.path("localify.sock"),
        }
    }

    fn command(&self, command: &str) -> Option<serde_json::Value> {
        let mut stream = UnixStream::connect(&self.socket).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        stream.write_all(format!("{command}\n").as_bytes()).ok()?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }

    fn stop(&mut self) {
        let _ = self.command(r#"{"cmd":"shutdown"}"#);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
    }
}

fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Sends what the Worker would, through the ingress, and returns the whole
/// response.
fn public_request(port: u16, secret: &str, url_id: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: m-m1.localify.dev\r\nConnection: close\r\n\
         X-Forwarded-For: 104.28.0.1\r\nX-Localify-Ingress: {secret}\r\n\
         X-Localify-Public-Host: {PUBLIC_HOST}\r\nX-Localify-Local-Host: {LOCAL_HOST}\r\n\
         X-Localify-Url-Id: {url_id}\r\nX-Localify-Client-Ip: {VISITOR}\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

fn process_alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn public_traffic_reaches_the_upstream_through_the_ingress() {
    let (heads_tx, heads) = mpsc::channel();
    let setup = set_up(upstream(heads_tx), fake_cloud());
    let mut daemon = Daemon::start(&setup);

    let status = wait_for("the daemon to serve the URL", || {
        let status = daemon.command(r#"{"cmd":"public_status"}"#)?;
        (status["data"]["servedUrls"] == serde_json::json!(["u1"])
            && setup.bin().join("pid").exists())
        .then_some(status)
    });
    assert_eq!(status["data"]["signedIn"], true);
    assert_eq!(status["data"]["tunnel"], "starting");

    // cloudflared runs with the token in its environment only, and with a
    // config of its own instead of ~/.cloudflared/config.yml.
    let args = std::fs::read_to_string(setup.bin().join("args")).unwrap();
    let config = setup.path("data").join("cloudflared.yml");
    let metrics = format!("127.0.0.1:{}", setup.ingress_port + 10);
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "--config",
            config.to_str().unwrap(),
            "tunnel",
            "--no-autoupdate",
            "--protocol",
            "http2",
            "--metrics",
            metrics.as_str(),
            "run"
        ]
    );
    assert!(setup.bin().join("token").exists());

    // Without the secret, nothing gets through.
    let refused = public_request(setup.ingress_port, "wrong", "u1", "/");
    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
    let unknown = public_request(setup.ingress_port, SECRET, "u2", "/");
    assert!(unknown.starts_with("HTTP/1.1 404"), "{unknown}");
    let action = public_request(
        setup.ingress_port,
        SECRET,
        "u1",
        "/.localify/open-in-editor",
    );
    assert!(action.starts_with("HTTP/1.1 404"), "{action}");

    // HTTP reaches the upstream with the local host and the public context.
    let response = public_request(setup.ingress_port, SECRET, "u1", "/hello?x=1");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let head = heads.recv_timeout(Duration::from_secs(5)).unwrap();
    let lower = head.to_ascii_lowercase();
    assert!(head.starts_with("GET /hello?x=1 HTTP/1.1\r\n"), "{head}");
    assert!(lower.contains(&format!("host: {LOCAL_HOST}\r\n")), "{head}");
    assert!(
        lower.contains(&format!("x-forwarded-host: {PUBLIC_HOST}\r\n")),
        "{head}"
    );
    assert!(lower.contains("x-forwarded-proto: https\r\n"), "{head}");
    assert!(lower.contains("x-forwarded-port: 443\r\n"), "{head}");
    assert!(
        lower.contains(&format!("x-forwarded-for: {VISITOR}\r\n")),
        "{head}"
    );
    assert!(
        lower.contains(&format!("x-localify-local-domain: {LOCAL_HOST}\r\n")),
        "{head}"
    );
    assert!(!lower.contains("x-localify-ingress"), "{head}");
    assert!(!head.contains(SECRET), "{head}");

    // A WebSocket upgrade goes through and carries data both ways.
    let mut socket = TcpStream::connect(("127.0.0.1", setup.ingress_port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let handshake = format!(
        "GET /_next/hmr HTTP/1.1\r\nHost: m-m1.localify.dev\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         X-Localify-Ingress: {SECRET}\r\nX-Localify-Public-Host: {PUBLIC_HOST}\r\n\
         X-Localify-Local-Host: {LOCAL_HOST}\r\nX-Localify-Url-Id: u1\r\n\
         X-Localify-Client-Ip: {VISITOR}\r\nOrigin: https://{PUBLIC_HOST}\r\n\r\n"
    );
    socket.write_all(handshake.as_bytes()).unwrap();
    let switched = read_head(&mut socket);
    assert!(switched.starts_with("HTTP/1.1 101"), "{switched}");
    let head = heads.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(head.contains(&format!("Host: {LOCAL_HOST}\r\n")), "{head}");
    // Next.js's HMR gets the page's own origin as the local host's.
    assert!(
        head.to_ascii_lowercase()
            .contains(&format!("origin: https://{LOCAL_HOST}\r\n")),
        "{head}"
    );
    assert!(
        head.contains(&format!("X-Forwarded-Host: {PUBLIC_HOST}\r\n")),
        "{head}"
    );
    assert!(
        head.contains(&format!("X-Forwarded-For: {VISITOR}\r\n")),
        "{head}"
    );
    socket.write_all(b"ping").unwrap();
    let mut echo = [0u8; 4];
    socket.read_exact(&mut echo).unwrap();
    assert_eq!(&echo, b"ping");
    drop(socket);

    let pid = std::fs::read_to_string(setup.bin().join("pid")).unwrap();
    daemon.stop();

    // Shutdown took cloudflared down with it.
    assert!(!process_alive(pid.trim()), "cloudflared {pid} still runs");
    let log = std::fs::read_to_string(setup.path("logs/localify.log")).unwrap();
    assert!(
        log.contains(&format!("source=public public_host={PUBLIC_HOST}")),
        "{log}"
    );
    assert!(!log.contains(SECRET) && !log.contains(TOKEN), "{log}");
    assert_eq!(
        std::fs::read_to_string(setup.path("logs/cloudflared.log")).unwrap(),
        ""
    );
}
