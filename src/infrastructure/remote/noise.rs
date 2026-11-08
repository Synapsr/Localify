//! `Noise_XX_25519_ChaChaPoly_BLAKE2s` through `snow`, for pairing and
//! every session (`docs/remote-machines.md`, "Sessions"; ADR 0029).
//!
//! The viewer is always the initiator. Handshake payloads are empty: the
//! three messages only carry keys. Once the handshake is done, each
//! direction has its own 64-bit nonce counter, and since frames arrive in
//! order over one socket, a frame that does not decrypt (repeated, missing,
//! reordered or forged) ends the session.

use std::fmt;

use anyhow::{Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::wire::{MAX_MESSAGE_BYTES, PROTO_MAX, Purpose};
use crate::infrastructure::launch_rev::random_bytes;

const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
/// The first bytes of every prologue; `1` is the session protocol version.
const PROLOGUE_LABEL: &[u8; 17] = b"localify-remote/1";
pub const KEY_LEN: usize = 32;
pub const HASH_LEN: usize = 32;
const TAG_LEN: usize = 16;
/// The longest Noise message.
const MAX_NOISE_MESSAGE: usize = 65535;

// The label carries the version as text; keep the two in step.
const _: () = assert!(PROTO_MAX == 1);

/// The 16 random bytes that name a session, chosen by the initiator.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId([u8; 16]);

impl SessionId {
    pub fn random() -> Result<Self> {
        let mut bytes = [0_u8; 16];
        random_bytes(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// base64url without padding, exactly 22 characters.
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 22 {
            return None;
        }
        let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
        Some(Self(bytes.try_into().ok()?))
    }

    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    pub fn bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({})", self.encode())
    }
}

/// Who and what a handshake is for. Every field goes into the prologue, so
/// a handshake replayed into another session, account, direction or
/// purpose fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContext {
    pub purpose: Purpose,
    pub account: String,
    pub initiator: String,
    pub responder: String,
    pub sid: SessionId,
}

impl SessionContext {
    /// The prologue, byte for byte as the contract gives it.
    pub fn prologue(&self) -> Result<Vec<u8>> {
        let mut prologue = Vec::with_capacity(17 + 1 + 3 * 64 + 16);
        prologue.extend_from_slice(PROLOGUE_LABEL);
        prologue.push(match self.purpose {
            Purpose::Session => 0x01,
            Purpose::Pair => 0x02,
        });
        for value in [&self.account, &self.initiator, &self.responder] {
            let length = u8::try_from(value.len())
                .map_err(|_| anyhow!("An id of {} bytes does not fit the prologue", value.len()))?;
            prologue.push(length);
            prologue.extend_from_slice(value.as_bytes());
        }
        prologue.extend_from_slice(self.sid.bytes());
        Ok(prologue)
    }
}

/// This machine's static X25519 key pair. The private half is wiped from
/// memory when the pair is dropped.
#[derive(PartialEq, Eq)]
pub struct StaticKeys {
    pub private: Zeroizing<[u8; KEY_LEN]>,
    pub public: [u8; KEY_LEN],
}

impl fmt::Debug for StaticKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticKeys")
            .field("private", &"<redacted>")
            .field("public", &encode_key(&self.public))
            .finish()
    }
}

impl StaticKeys {
    pub fn generate() -> Result<Self> {
        let mut keypair = builder()?
            .generate_keypair()
            .map_err(|error| anyhow!("Could not generate a remote key: {error}"))?;
        let private = key_bytes(&keypair.private).map(Zeroizing::new);
        keypair.private.zeroize();
        Ok(Self {
            private: private?,
            public: key_bytes(&keypair.public)?,
        })
    }
}

fn builder() -> Result<snow::Builder<'static>> {
    let params = NOISE_PARAMS
        .parse()
        .map_err(|error| anyhow!("{NOISE_PARAMS} is not supported: {error}"))?;
    Ok(snow::Builder::new(params))
}

fn key_bytes(bytes: &[u8]) -> Result<[u8; KEY_LEN]> {
    bytes
        .try_into()
        .map_err(|_| anyhow!("A remote key is {KEY_LEN} bytes, not {}", bytes.len()))
}

/// base64url without padding.
pub fn encode_key(key: &[u8; KEY_LEN]) -> String {
    URL_SAFE_NO_PAD.encode(key)
}

/// Decodes a key. The error never quotes the input: it may be a private
/// key.
pub fn decode_key(text: &str) -> Result<[u8; KEY_LEN]> {
    let bytes = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(text)
            .map_err(|_| anyhow!("not {KEY_LEN} bytes of base64url without padding"))?,
    );
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("not {KEY_LEN} bytes of base64url without padding"))
}

/// Decodes a private key straight into memory that is wiped when dropped,
/// on failure too: no copy of it is left in an ordinary buffer.
pub fn decode_private_key(text: &str) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    // 43 characters decode to 32 bytes; the slack lets `decode_slice`
    // accept its own estimate of the length.
    let mut buffer = Zeroizing::new([0_u8; KEY_LEN + 4]);
    let length = URL_SAFE_NO_PAD
        .decode_slice(text, buffer.as_mut_slice())
        .map_err(|_| anyhow!("not {KEY_LEN} bytes of base64url without padding"))?;
    if length != KEY_LEN {
        bail!("not {KEY_LEN} bytes of base64url without padding");
    }
    let mut key = Zeroizing::new([0_u8; KEY_LEN]);
    key.copy_from_slice(&buffer[..KEY_LEN]);
    Ok(key)
}

/// The first 8 bytes of SHA-256 of the key, as `1a2b 3c4d 5e6f 7a8b`.
pub fn fingerprint(key: &[u8; KEY_LEN]) -> String {
    let digest = Sha256::digest(key);
    digest[..8]
        .chunks(2)
        .map(|pair| format!("{:02x}{:02x}", pair[0], pair[1]))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A handshake in progress.
pub struct Handshake {
    state: snow::HandshakeState,
}

impl Handshake {
    pub fn initiator(keys: &StaticKeys, context: &SessionContext) -> Result<Self> {
        Self::build(keys, context, true)
    }

    pub fn responder(keys: &StaticKeys, context: &SessionContext) -> Result<Self> {
        Self::build(keys, context, false)
    }

    fn build(keys: &StaticKeys, context: &SessionContext, initiator: bool) -> Result<Self> {
        let prologue = context.prologue()?;
        let builder = builder()?
            .local_private_key(keys.private.as_slice())
            .and_then(|builder| builder.prologue(&prologue))
            .map_err(|error| anyhow!("Could not set up the handshake: {error}"))?;
        let state = if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .map_err(|error| anyhow!("Could not set up the handshake: {error}"))?;
        Ok(Self { state })
    }

    /// The next handshake message, with an empty payload.
    pub fn write(&mut self) -> Result<Vec<u8>> {
        let mut message = vec![0_u8; 256];
        let length = self
            .state
            .write_message(&[], &mut message)
            .map_err(|error| anyhow!("Handshake write failed: {error}"))?;
        message.truncate(length);
        Ok(message)
    }

    /// Reads the peer's handshake message. A payload is refused: this
    /// protocol never sends one.
    pub fn read(&mut self, message: &[u8]) -> Result<()> {
        let mut payload = vec![0_u8; MAX_NOISE_MESSAGE];
        let length = self
            .state
            .read_message(message, &mut payload)
            .map_err(|error| anyhow!("Handshake read failed: {error}"))?;
        if length != 0 {
            bail!("A handshake message carried a payload");
        }
        Ok(())
    }

    /// Whether this side writes the next message.
    pub fn is_my_turn(&self) -> bool {
        self.state.is_my_turn()
    }

    pub fn is_finished(&self) -> bool {
        self.state.is_handshake_finished()
    }

    /// The peer's static key, once its handshake message carried it.
    pub fn remote_static(&self) -> Option<[u8; KEY_LEN]> {
        self.state
            .get_remote_static()
            .and_then(|key| key.try_into().ok())
    }

    /// The transport, and the final handshake hash `h` that pairing needs.
    pub fn finish(self) -> Result<(Transport, [u8; HASH_LEN])> {
        let hash: [u8; HASH_LEN] = self
            .state
            .get_handshake_hash()
            .try_into()
            .map_err(|_| anyhow!("The handshake hash is not {HASH_LEN} bytes"))?;
        let state = self
            .state
            .into_transport_mode()
            .map_err(|error| anyhow!("The handshake is not finished: {error}"))?;
        Ok((Transport { state }, hash))
    }
}

/// A finished handshake: encrypts and decrypts in strict order.
pub struct Transport {
    state: snow::TransportState,
}

impl Transport {
    /// Encrypts one session message of at most 32 KiB.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        if plaintext.len() > MAX_MESSAGE_BYTES {
            bail!(
                "A session message of {} bytes is over the {MAX_MESSAGE_BYTES}-byte limit",
                plaintext.len()
            );
        }
        let mut message = vec![0_u8; plaintext.len() + TAG_LEN];
        let length = self
            .state
            .write_message(plaintext, &mut message)
            .map_err(|error| anyhow!("Encryption failed: {error}"))?;
        message.truncate(length);
        Ok(message)
    }

    /// Decrypts the next message. Anything but the next one in order fails,
    /// and the caller ends the session.
    pub fn decrypt(&mut self, message: &[u8]) -> Result<Vec<u8>> {
        if message.len() < TAG_LEN || message.len() > MAX_MESSAGE_BYTES + TAG_LEN {
            bail!(
                "A session frame of {} bytes is out of bounds",
                message.len()
            );
        }
        let mut plaintext = vec![0_u8; message.len()];
        let length = self
            .state
            .read_message(message, &mut plaintext)
            .map_err(|error| anyhow!("Decryption failed: {error}"))?;
        plaintext.truncate(length);
        Ok(plaintext)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn context(purpose: Purpose) -> SessionContext {
        SessionContext {
            purpose,
            account: "u1".into(),
            initiator: "mA".into(),
            responder: "mB".into(),
            sid: SessionId([7; 16]),
        }
    }

    /// A finished side: its transport and the handshake hash.
    type Side = (Transport, [u8; 32]);

    /// Runs the three XX messages between two fresh handshakes.
    pub fn handshake(
        a: &StaticKeys,
        b: &StaticKeys,
        ctx_a: &SessionContext,
        ctx_b: &SessionContext,
    ) -> Result<(Side, Side)> {
        let mut initiator = Handshake::initiator(a, ctx_a)?;
        let mut responder = Handshake::responder(b, ctx_b)?;
        responder.read(&initiator.write()?)?;
        initiator.read(&responder.write()?)?;
        responder.read(&initiator.write()?)?;
        Ok((initiator.finish()?, responder.finish()?))
    }

    #[test]
    fn the_prologue_is_encoded_byte_for_byte() {
        let prologue = context(Purpose::Pair).prologue().unwrap();
        let mut expected = b"localify-remote/1".to_vec();
        expected.push(0x02);
        expected.extend_from_slice(&[2, b'u', b'1', 2, b'm', b'A', 2, b'm', b'B']);
        expected.extend_from_slice(&[7; 16]);
        assert_eq!(prologue, expected);
        assert_eq!(context(Purpose::Session).prologue().unwrap()[17], 0x01);

        let mut long = context(Purpose::Pair);
        long.account = "x".repeat(256);
        assert!(long.prologue().is_err());
    }

    #[test]
    fn both_sides_learn_each_other_and_share_one_hash() {
        let (a, b) = (
            StaticKeys::generate().unwrap(),
            StaticKeys::generate().unwrap(),
        );
        let ctx = context(Purpose::Session);
        let mut initiator = Handshake::initiator(&a, &ctx).unwrap();
        let mut responder = Handshake::responder(&b, &ctx).unwrap();
        responder.read(&initiator.write().unwrap()).unwrap();
        initiator.read(&responder.write().unwrap()).unwrap();
        assert_eq!(initiator.remote_static(), Some(b.public));
        responder.read(&initiator.write().unwrap()).unwrap();
        assert_eq!(responder.remote_static(), Some(a.public));
        let ((mut ta, ha), (mut tb, hb)) =
            (initiator.finish().unwrap(), responder.finish().unwrap());
        assert_eq!(ha, hb);

        let sealed = ta.encrypt(b"hello").unwrap();
        assert_eq!(tb.decrypt(&sealed).unwrap(), b"hello");
        let back = tb.encrypt(b"kept").unwrap();
        assert_eq!(ta.decrypt(&back).unwrap(), b"kept");
    }

    #[test]
    fn any_change_to_the_context_breaks_the_handshake() {
        let (a, b) = (
            StaticKeys::generate().unwrap(),
            StaticKeys::generate().unwrap(),
        );
        let ours = context(Purpose::Session);
        let mut other_purpose = ours.clone();
        other_purpose.purpose = Purpose::Pair;
        let mut other_account = ours.clone();
        other_account.account = "u2".into();
        let mut reversed = ours.clone();
        std::mem::swap(&mut reversed.initiator, &mut reversed.responder);
        let mut other_session = ours.clone();
        other_session.sid = SessionId([8; 16]);
        for theirs in [other_purpose, other_account, reversed, other_session] {
            assert!(handshake(&a, &b, &ours, &theirs).is_err(), "{theirs:?}");
        }
        assert!(handshake(&a, &b, &ours, &ours).is_ok());
    }

    #[test]
    fn a_repeated_reordered_or_altered_frame_fails() {
        let (a, b) = (
            StaticKeys::generate().unwrap(),
            StaticKeys::generate().unwrap(),
        );
        let ctx = context(Purpose::Session);
        let ((mut ta, _), (mut tb, _)) = handshake(&a, &b, &ctx, &ctx).unwrap();
        let first = ta.encrypt(b"one").unwrap();
        let second = ta.encrypt(b"two").unwrap();
        tb.decrypt(&first).unwrap();
        // Replayed: its nonce is behind the counter.
        assert!(tb.decrypt(&first).is_err());

        let ((mut ta, _), (mut tb, _)) = handshake(&a, &b, &ctx, &ctx).unwrap();
        let first = ta.encrypt(b"one").unwrap();
        let second_fresh = ta.encrypt(b"two").unwrap();
        // Skipped: the counter expects the first.
        assert!(tb.decrypt(&second_fresh).is_err());
        let mut altered = first.clone();
        altered[0] ^= 1;
        let ((_, _), (mut tc, _)) = handshake(&a, &b, &ctx, &ctx).unwrap();
        assert!(tc.decrypt(&altered).is_err());
        // From another session: fresh ephemeral keys, so nothing decrypts.
        assert!(tc.decrypt(&second).is_err());
    }

    #[test]
    fn a_forced_nonce_reuse_is_rejected() {
        let (a, b) = (
            StaticKeys::generate().unwrap(),
            StaticKeys::generate().unwrap(),
        );
        let ctx = context(Purpose::Session);
        let mut initiator = Handshake::initiator(&a, &ctx).unwrap();
        let mut responder = Handshake::responder(&b, &ctx).unwrap();
        responder.read(&initiator.write().unwrap()).unwrap();
        initiator.read(&responder.write().unwrap()).unwrap();
        responder.read(&initiator.write().unwrap()).unwrap();
        // A sender that ignores its counter, as a modified client could.
        let forced = initiator.state.into_stateless_transport_mode().unwrap();
        let (mut receiver, _) = responder.finish().unwrap();
        let sealed = |text: &[u8]| {
            let mut out = vec![0; text.len() + TAG_LEN];
            let length = forced.write_message(0, text, &mut out).unwrap();
            out.truncate(length);
            out
        };
        let first = sealed(b"keep");
        let reused = sealed(b"other");
        receiver.decrypt(&first).unwrap();
        assert!(receiver.decrypt(&reused).is_err());
    }

    #[test]
    fn keys_and_ids_round_trip_and_reject_padding() {
        let keys = StaticKeys::generate().unwrap();
        let text = encode_key(&keys.public);
        assert_eq!(text.len(), 43);
        assert_eq!(decode_key(&text).unwrap(), keys.public);
        assert!(decode_key(&format!("{text}=")).is_err());
        assert!(decode_key("short").is_err());

        let sid = SessionId::random().unwrap();
        assert_eq!(sid.encode().len(), 22);
        assert_eq!(SessionId::parse(&sid.encode()), Some(sid));
        assert_eq!(SessionId::parse("too-short"), None);
        assert!(!format!("{keys:?}").contains(&encode_key(&keys.private)));
        let secret = encode_key(&keys.private);
        let error = format!("{:#}", decode_key(&format!("{secret}!")).unwrap_err());
        assert!(!error.contains(&secret[..8]), "{error}");
        assert_eq!(*decode_private_key(&secret).unwrap(), *keys.private);
        for bad in [
            format!("{secret}!"),
            format!("{secret}AAAA"),
            secret[..40].to_string(),
            String::new(),
        ] {
            let error = format!("{:#}", decode_private_key(&bad).unwrap_err());
            assert!(!error.contains(&secret[..8]), "{error}");
        }
    }

    #[test]
    fn a_fingerprint_is_four_groups_of_four_hex_digits() {
        let print = fingerprint(&[0; 32]);
        // SHA-256 of 32 zero bytes starts 66687aadf862bd77.
        assert_eq!(print, "6668 7aad f862 bd77");
    }
}
