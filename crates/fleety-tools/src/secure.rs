//! The authenticated, encrypted control channel a paired profile uses.
//!
//! A saved profile stores a per-device token and the Server identity it was
//! pinned to. Before this module existed, a client proved *nothing* about the
//! peer before handing over that token: it connected, sent `Hello { token }`,
//! and only then compared the `Welcome` fingerprint — a public UUID that mDNS
//! already broadcasts. Anyone who took over a saved address collected the
//! token and could forge the reply that authorizes control frames.
//!
//! Here both sides run a Noise `NNpsk0` handshake keyed by that same token, so
//! neither can continue without already holding it. The token itself never
//! reaches the wire, the Server proves its identity before the client commits
//! to anything, and every later frame travels sealed and integrity-protected —
//! which also covers the provider keys and OAuth credentials that config frames
//! carry.
//!
//! Deliberately *not* covered: pairing and same-host loopback trust, which have
//! no pre-shared token to key a handshake with. Those keep their explicit
//! endpoint plus pairing code, and a paired reconnect never falls back to them.

use agent_core::{CoreError, Result};
use base64::Engine as _;

/// Wire version of this channel. It is bound into the handshake prologue, so a
/// tampered advertisement makes the handshake fail rather than silently
/// negotiating something weaker.
pub const SECURE_CHANNEL_VERSION: u32 = 1;

const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
const PSK_SALT: &[u8] = b"fleety-secure-channel-psk-v1";
const KEY_ID_SALT: &[u8] = b"fleety-secure-channel-id-v1";
const PROLOGUE_DOMAIN: &[u8] = b"fleety-secure-channel-prologue-v1";

/// A Noise transport message carries at most 65535 bytes including its 16-byte
/// authentication tag, so a control frame larger than this is split.
const MAX_NOISE_PAYLOAD: usize = 65535 - 16;

/// Ceiling on a single reassembled control frame. Generous for tool output and
/// config snapshots, but bounded so a hostile peer cannot make us allocate
/// without limit.
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn crypto_error(context: &str) -> CoreError {
    // Handshake and AEAD failures are never described in detail: the peer is
    // unauthenticated by definition, and the distinction between "wrong key"
    // and "malformed input" is exactly what an attacker probes for.
    CoreError::Message(format!(
        "the Server did not prove it holds this profile's credential ({context})"
    ))
}

fn derive(salt: &[u8], token: &str, server_fingerprint: &str, out: &mut [u8]) -> Result<()> {
    hkdf::Hkdf::<sha2::Sha256>::new(Some(salt), token.as_bytes())
        .expand(server_fingerprint.as_bytes(), out)
        .map_err(|_| CoreError::Message("could not derive the secure channel key".to_string()))
}

/// The 32-byte pre-shared key both sides feed the handshake. Binding the pinned
/// Server identity in means a token stolen from one Server cannot open a
/// channel to a different one.
pub fn derive_psk(token: &str, server_fingerprint: &str) -> Result<[u8; 32]> {
    let mut psk = [0u8; 32];
    derive(PSK_SALT, token, server_fingerprint, &mut psk)?;
    Ok(psk)
}

/// A public, stable handle the Server uses to find *which* device token to key
/// the handshake with. Derived under a separate salt, so it reveals neither the
/// token nor the pre-shared key, and it is bound to one Server identity.
///
/// It is deliberately stable, and travels in the clear as the first field of
/// every opening: that is the cost of letting a Server pick one pre-shared key
/// out of many without a round trip. An observer can therefore recognise the
/// same device across networks, and someone replaying a captured opening can
/// learn whether a given Server has that device enrolled. Neither reveals a
/// credential. Rotating it would mean binding it to a time window both sides
/// agree on, which is a change to the handshake, not to this function.
pub fn key_id(token: &str, server_fingerprint: &str) -> Result<String> {
    let mut id = [0u8; 16];
    derive(KEY_ID_SALT, token, server_fingerprint, &mut id)?;
    Ok(id.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Everything the two sides must already agree on before the first Noise
/// message. Noise mixes the prologue into the handshake hash, so any
/// disagreement — a downgraded version, a swapped key hint — aborts the
/// handshake instead of being negotiated away.
fn prologue(version: u32, key_id: &str) -> Vec<u8> {
    let mut prologue = Vec::with_capacity(PROLOGUE_DOMAIN.len() + 4 + key_id.len());
    prologue.extend_from_slice(PROLOGUE_DOMAIN);
    prologue.extend_from_slice(&version.to_be_bytes());
    prologue.extend_from_slice(key_id.as_bytes());
    prologue
}

/// Which side of the handshake to build. `snow`'s builder borrows the prologue
/// and the key, so the state has to be built here rather than handed back.
enum Role {
    Initiator,
    Responder,
}

fn handshake(
    version: u32,
    token: &str,
    server_fingerprint: &str,
    role: Role,
) -> Result<snow::HandshakeState> {
    if token.trim().is_empty() || server_fingerprint.trim().is_empty() {
        return Err(CoreError::Message(
            "a secure channel needs both a paired token and the pinned Server identity".to_string(),
        ));
    }
    let params = NOISE_PARAMS
        .parse()
        .map_err(|_| CoreError::Message("invalid secure channel parameters".to_string()))?;
    let psk = derive_psk(token, server_fingerprint)?;
    let key_id = key_id(token, server_fingerprint)?;
    let prologue = prologue(version, &key_id);
    let builder = snow::Builder::new(params)
        .prologue(&prologue)
        .and_then(|builder| builder.psk(0, &psk))
        .map_err(|_| CoreError::Message("could not prepare the secure channel".to_string()))?;
    match role {
        Role::Initiator => builder.build_initiator(),
        Role::Responder => builder.build_responder(),
    }
    .map_err(|_| CoreError::Message("could not start the secure channel".to_string()))
}

/// The client half of the handshake.
pub struct Initiator {
    state: snow::HandshakeState,
}

impl Initiator {
    /// Start a handshake, returning the first message to put on the wire.
    pub fn start(token: &str, server_fingerprint: &str) -> Result<(Self, String)> {
        let mut state = handshake(
            SECURE_CHANNEL_VERSION,
            token,
            server_fingerprint,
            Role::Initiator,
        )?;
        let mut buf = vec![0u8; MAX_NOISE_PAYLOAD];
        let len = state
            .write_message(&[], &mut buf)
            .map_err(|_| CoreError::Message("could not start the secure channel".to_string()))?;
        buf.truncate(len);
        Ok((Self { state }, b64().encode(&buf)))
    }

    /// Verify the Server's reply. Succeeding here *is* the proof that the peer
    /// holds this profile's token; failing means we never reveal anything.
    pub fn finish(mut self, version: u32, reply: &str) -> Result<SecureSession> {
        if version != SECURE_CHANNEL_VERSION {
            return Err(crypto_error("unsupported channel version"));
        }
        let message = b64()
            .decode(reply)
            .map_err(|_| crypto_error("malformed reply"))?;
        let mut buf = vec![0u8; MAX_NOISE_PAYLOAD];
        self.state
            .read_message(&message, &mut buf)
            .map_err(|_| crypto_error("rejected reply"))?;
        SecureSession::from_handshake(self.state)
    }
}

/// The Server half of the handshake.
pub struct Responder {
    state: snow::HandshakeState,
}

impl Responder {
    /// Answer a client's opening message with the token that the announced
    /// `key_id` selected. Returns the reply to send back.
    pub fn accept(
        version: u32,
        token: &str,
        server_fingerprint: &str,
        opening: &str,
    ) -> Result<(Self, String)> {
        if version != SECURE_CHANNEL_VERSION {
            return Err(crypto_error("unsupported channel version"));
        }
        let message = b64()
            .decode(opening)
            .map_err(|_| crypto_error("malformed opening"))?;
        let mut state = handshake(version, token, server_fingerprint, Role::Responder)?;
        let mut buf = vec![0u8; MAX_NOISE_PAYLOAD];
        state
            .read_message(&message, &mut buf)
            .map_err(|_| crypto_error("rejected opening"))?;
        let mut reply = vec![0u8; MAX_NOISE_PAYLOAD];
        let len = state
            .write_message(&[], &mut reply)
            .map_err(|_| crypto_error("could not answer"))?;
        reply.truncate(len);
        Ok((Self { state }, b64().encode(&reply)))
    }

    pub fn finish(self) -> Result<SecureSession> {
        SecureSession::from_handshake(self.state)
    }
}

/// An established channel. Every control frame from here on is sealed, so a
/// peer that only relays bytes can neither read a frame nor insert one.
pub struct SecureSession {
    transport: snow::TransportState,
}

impl SecureSession {
    fn from_handshake(state: snow::HandshakeState) -> Result<Self> {
        if !state.is_handshake_finished() {
            return Err(crypto_error("incomplete handshake"));
        }
        let transport = state
            .into_transport_mode()
            .map_err(|_| crypto_error("could not establish the channel"))?;
        Ok(Self { transport })
    }

    /// Seal one control frame. Frames larger than a single Noise message are
    /// split; each part is sealed under its own counter, so a dropped,
    /// reordered, or replayed part breaks the channel instead of passing.
    pub fn seal(&mut self, frame: &str) -> Result<String> {
        let plaintext = frame.as_bytes();
        let mut wire = Vec::with_capacity(plaintext.len() + 32);
        let mut buf = vec![0u8; MAX_NOISE_PAYLOAD + 16];
        for chunk in plaintext.chunks(MAX_NOISE_PAYLOAD) {
            let len = self
                .transport
                .write_message(chunk, &mut buf)
                .map_err(|_| CoreError::Message("could not seal a control frame".to_string()))?;
            let len_u32 = u32::try_from(len)
                .map_err(|_| CoreError::Message("could not seal a control frame".to_string()))?;
            wire.extend_from_slice(&len_u32.to_be_bytes());
            wire.extend_from_slice(&buf[..len]);
        }
        // An empty frame still produces one sealed part, so "no parts" can never
        // be a valid encoding an attacker could forge cheaply.
        if plaintext.is_empty() {
            let len = self
                .transport
                .write_message(&[], &mut buf)
                .map_err(|_| CoreError::Message("could not seal a control frame".to_string()))?;
            let len_u32 = u32::try_from(len)
                .map_err(|_| CoreError::Message("could not seal a control frame".to_string()))?;
            wire.extend_from_slice(&len_u32.to_be_bytes());
            wire.extend_from_slice(&buf[..len]);
        }
        Ok(b64().encode(&wire))
    }

    /// Open one sealed control frame. Any tampering, truncation, replay, or
    /// reordering fails here, and the caller must drop the connection.
    pub fn open(&mut self, payload: &str) -> Result<String> {
        let wire = b64()
            .decode(payload)
            .map_err(|_| crypto_error("malformed control frame"))?;
        if wire.is_empty() {
            // No parts means nothing was authenticated; an empty frame is a
            // forgery a peer could produce without holding any key.
            return Err(crypto_error("empty control frame"));
        }
        let mut plaintext = Vec::new();
        let mut buf = vec![0u8; MAX_NOISE_PAYLOAD + 16];
        let mut cursor = 0usize;
        while cursor < wire.len() {
            let header_end = cursor
                .checked_add(4)
                .filter(|end| *end <= wire.len())
                .ok_or_else(|| crypto_error("truncated control frame"))?;
            let mut header = [0u8; 4];
            header.copy_from_slice(&wire[cursor..header_end]);
            let len = u32::from_be_bytes(header) as usize;
            if len == 0 || len > MAX_NOISE_PAYLOAD + 16 {
                return Err(crypto_error("invalid control frame part"));
            }
            let body_end = header_end
                .checked_add(len)
                .filter(|end| *end <= wire.len())
                .ok_or_else(|| crypto_error("truncated control frame"))?;
            let written = self
                .transport
                .read_message(&wire[header_end..body_end], &mut buf)
                .map_err(|_| crypto_error("rejected control frame"))?;
            if plaintext.len().saturating_add(written) > MAX_FRAME_BYTES {
                return Err(crypto_error("oversized control frame"));
            }
            plaintext.extend_from_slice(&buf[..written]);
            cursor = body_end;
        }
        String::from_utf8(plaintext).map_err(|_| crypto_error("control frame was not text"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "device-token-9f3a";
    const FINGERPRINT: &str = "3f1c9a52-0d44-4a51-9b6e-1f2d3c4b5a60";

    /// Drive a full handshake between two honest peers.
    fn paired_channel() -> (SecureSession, SecureSession) {
        let (initiator, opening) = Initiator::start(TOKEN, FINGERPRINT).expect("start handshake");
        let (responder, reply) =
            Responder::accept(SECURE_CHANNEL_VERSION, TOKEN, FINGERPRINT, &opening)
                .expect("server accepts the opening");
        let client = initiator
            .finish(SECURE_CHANNEL_VERSION, &reply)
            .expect("client verifies the server");
        let server = responder.finish().expect("server establishes the channel");
        (client, server)
    }

    #[test]
    fn a_paired_channel_round_trips_control_frames_in_both_directions() {
        let (mut client, mut server) = paired_channel();

        let sealed = client
            .seal(r#"{"type":"hello","token":"device-token-9f3a"}"#)
            .expect("seal");
        assert!(
            !sealed.contains("device-token-9f3a"),
            "the sealed frame must not carry the credential in the clear"
        );
        assert_eq!(
            server.open(&sealed).expect("server opens the frame"),
            r#"{"type":"hello","token":"device-token-9f3a"}"#
        );

        let sealed = server.seal(r#"{"type":"welcome"}"#).expect("seal");
        assert_eq!(
            client.open(&sealed).expect("client opens the frame"),
            r#"{"type":"welcome"}"#
        );
    }

    #[test]
    fn a_peer_without_the_device_token_cannot_complete_the_handshake() {
        let (_, opening) = Initiator::start(TOKEN, FINGERPRINT).expect("start handshake");

        let rejected = Responder::accept(
            SECURE_CHANNEL_VERSION,
            "some-other-device-token",
            FINGERPRINT,
            &opening,
        );

        assert!(
            rejected.is_err(),
            "a Server that does not hold this token must not be able to answer"
        );
    }

    #[test]
    fn a_different_server_identity_cannot_complete_the_handshake() {
        let (_, opening) = Initiator::start(TOKEN, FINGERPRINT).expect("start handshake");

        // The impostor holds the token but is a different Server — credentials
        // copied onto another host must still be refused.
        let rejected = Responder::accept(
            SECURE_CHANNEL_VERSION,
            TOKEN,
            "11111111-2222-3333-4444-555555555555",
            &opening,
        );

        assert!(
            rejected.is_err(),
            "a Server with a different pinned identity must not be able to answer"
        );
    }

    #[test]
    fn a_client_pinned_to_another_server_refuses_the_reply() {
        let (initiator, opening) = Initiator::start(TOKEN, FINGERPRINT).expect("start handshake");
        // The honest Server for a *different* pin answers a relayed opening.
        let relayed = Initiator::start(TOKEN, "11111111-2222-3333-4444-555555555555")
            .expect("start against the other Server")
            .1;
        let (_, reply) = Responder::accept(
            SECURE_CHANNEL_VERSION,
            TOKEN,
            "11111111-2222-3333-4444-555555555555",
            &relayed,
        )
        .expect("the other Server answers its own client");
        let _ = opening;

        assert!(
            initiator.finish(SECURE_CHANNEL_VERSION, &reply).is_err(),
            "a reply produced for another Server's session must not be accepted"
        );
    }

    #[test]
    fn a_downgraded_channel_version_aborts_the_handshake() {
        let (initiator, opening) = Initiator::start(TOKEN, FINGERPRINT).expect("start handshake");

        assert!(
            Responder::accept(SECURE_CHANNEL_VERSION + 1, TOKEN, FINGERPRINT, &opening).is_err(),
            "a version the client did not offer must not be accepted"
        );
        assert!(
            initiator
                .finish(SECURE_CHANNEL_VERSION + 1, "AAAA")
                .is_err(),
            "the client must refuse a reply announcing a different version"
        );
    }

    /// The explicit version comparison is the first gate, but it is only a
    /// check on a value an attacker controls. This proves the second gate: the
    /// version is mixed into the handshake itself, so two peers that disagree
    /// derive different state and cannot talk at all.
    #[test]
    fn the_channel_version_is_bound_into_the_handshake_not_merely_compared() {
        let mut client =
            handshake(2, TOKEN, FINGERPRINT, Role::Initiator).expect("client offers version 2");
        let mut opening = vec![0u8; MAX_NOISE_PAYLOAD];
        let len = client
            .write_message(&[], &mut opening)
            .expect("client opening");

        // A Server that believes it is speaking version 1 holds the same token
        // and the same identity — only the negotiated version differs.
        let mut server =
            handshake(1, TOKEN, FINGERPRINT, Role::Responder).expect("server assumes version 1");
        let mut buf = vec![0u8; MAX_NOISE_PAYLOAD];

        assert!(
            server.read_message(&opening[..len], &mut buf).is_err(),
            "a version mismatch must break the handshake, not be negotiated away"
        );
    }

    #[test]
    fn a_tampered_sealed_frame_is_rejected() {
        let (mut client, mut server) = paired_channel();
        let sealed = client.seal(r#"{"type":"run_tool"}"#).expect("seal");

        let mut wire = b64().decode(&sealed).expect("decode");
        let last = wire.len() - 1;
        wire[last] ^= 0x01;
        let tampered = b64().encode(&wire);

        assert!(
            server.open(&tampered).is_err(),
            "a relay that flips a byte must not be able to alter a control frame"
        );
    }

    #[test]
    fn a_replayed_sealed_frame_is_rejected() {
        let (mut client, mut server) = paired_channel();
        let first = client
            .seal(r#"{"type":"run_tool","call_id":"1"}"#)
            .expect("seal");
        let second = client
            .seal(r#"{"type":"run_tool","call_id":"2"}"#)
            .expect("seal");

        server.open(&first).expect("first frame opens");
        server.open(&second).expect("second frame opens");

        assert!(
            server.open(&second).is_err(),
            "a relay must not be able to replay a control frame it captured"
        );
    }

    #[test]
    fn a_frame_larger_than_one_noise_message_round_trips() {
        let (mut client, mut server) = paired_channel();
        let big = "x".repeat(MAX_NOISE_PAYLOAD * 3 + 17);

        let sealed = client.seal(&big).expect("seal a large frame");
        assert_eq!(server.open(&sealed).expect("open a large frame"), big);
    }

    #[test]
    fn an_empty_frame_is_refused_rather_than_opened_as_nothing() {
        let (_, mut server) = paired_channel();
        assert!(
            server.open("").is_err(),
            "an unauthenticated empty frame must not open as an empty message"
        );
    }

    #[test]
    fn truncated_and_malformed_input_is_refused_without_panicking() {
        let (mut client, mut server) = paired_channel();
        let sealed = client.seal(r#"{"type":"welcome"}"#).expect("seal");
        let wire = b64().decode(&sealed).expect("decode");

        for cut in 0..wire.len() {
            let truncated = b64().encode(&wire[..cut]);
            let _ = server.open(&truncated);
        }
        assert!(server.open("not base64 !!").is_err());
        assert!(server
            .open(&b64().encode([0xff, 0xff, 0xff, 0xff]))
            .is_err());
    }

    #[test]
    fn the_key_hint_reveals_neither_the_token_nor_the_pre_shared_key() {
        let hint = key_id(TOKEN, FINGERPRINT).expect("key id");
        let psk = derive_psk(TOKEN, FINGERPRINT).expect("psk");

        assert!(!hint.contains(TOKEN));
        // With one salt for both, HKDF makes the shorter output a prefix of the
        // longer one, so the hint would publish the first half of the key on
        // every connection. Compare like for like, or the check cannot fail.
        let psk_prefix: String = psk[..8].iter().map(|byte| format!("{byte:02x}")).collect();
        assert!(
            !hint.starts_with(&psk_prefix),
            "the public hint must not be derived from the same secret material as the key"
        );
        assert_eq!(hint, key_id(TOKEN, FINGERPRINT).expect("stable"));
        assert_ne!(
            hint,
            key_id(TOKEN, "11111111-2222-3333-4444-555555555555").expect("other server"),
            "the same token on a different Server must not share a key hint"
        );
        assert_ne!(
            psk,
            derive_psk(TOKEN, "11111111-2222-3333-4444-555555555555").expect("other server"),
            "the same token on a different Server must not share a key"
        );
    }

    #[test]
    fn an_empty_token_or_identity_is_refused() {
        assert!(Initiator::start("", FINGERPRINT).is_err());
        assert!(Initiator::start(TOKEN, "  ").is_err());
    }
}
