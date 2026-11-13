//! Client-side key exchange. The relay handles public keys and ciphertext only.
//!
//! Membership/public-key authenticity relies on the application's authenticated
//! admission service. This does not provide out-of-band identity verification
//! against a malicious admission server. Open rooms admit any authorized joiner.
use anyhow::{Result, anyhow, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::{PublicKey, SecretKey, elliptic_curve::sec1::ToEncodedPoint};
use rand::{RngCore, rngs::OsRng};
use ring::{aead, hkdf};
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroizing;

pub const PROTOCOL: u8 = 1;
pub const MAX_PEERS: usize = 64;
const WRAP_INFO: &[u8] = b"telinha-key-wrap-v1";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayCredentials {
    pub protocol: u8,
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Peer {
    pub identity: String,
    pub public_key: String,
    pub can_publish: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Envelope {
    pub sender: String,
    pub recipient: String,
    pub generation: u32,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayState {
    pub protocol: u8,
    pub peers: Vec<Peer>,
    pub envelopes: Vec<Envelope>,
}

fn decode<const N: usize>(value: &str) -> Result<[u8; N]> {
    ensure!(value.len() == (N * 8).div_ceil(6), "Invalid encoded length");
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(value)?);
    ensure!(
        URL_SAFE_NO_PAD.encode(&bytes) == value,
        "Noncanonical encoding"
    );
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("Invalid byte length"))
}

pub fn valid_public_key(value: &str) -> bool {
    decode::<65>(value)
        .is_ok_and(|bytes| bytes[0] == 4 && PublicKey::from_sec1_bytes(&bytes).is_ok())
}

pub fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_'))
}

pub fn valid_envelope(envelope: &Envelope) -> bool {
    valid_identity(&envelope.sender)
        && valid_identity(&envelope.recipient)
        && envelope.sender != envelope.recipient
        && decode::<12>(&envelope.nonce).is_ok()
        && decode::<48>(&envelope.ciphertext).is_ok()
}

pub struct Identity(SecretKey);

impl Default for Identity {
    fn default() -> Self {
        Self::new()
    }
}

impl Identity {
    pub fn new() -> Self {
        Self(SecretKey::random(&mut OsRng))
    }

    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0.public_key().to_encoded_point(false).as_bytes())
    }

    pub fn peer(&self, identity: String) -> Peer {
        Peer {
            identity,
            public_key: self.public_key(),
            can_publish: true,
        }
    }

    fn wrapping_key(&self, peer: &Peer, context: &[u8]) -> Result<aead::LessSafeKey> {
        let public = PublicKey::from_sec1_bytes(&decode::<65>(&peer.public_key)?)
            .map_err(|_| anyhow!("Invalid peer public key"))?;
        let secret = p256::ecdh::diffie_hellman(self.0.to_nonzero_scalar(), public.as_affine());
        struct KeyLength;
        impl hkdf::KeyType for KeyLength {
            fn len(&self) -> usize {
                32
            }
        }
        let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, context);
        let prk = salt.extract(secret.raw_secret_bytes());
        let mut bytes = Zeroizing::new([0u8; 32]);
        prk.expand(&[WRAP_INFO], KeyLength)
            .map_err(|_| anyhow!("Key derivation failed"))?
            .fill(bytes.as_mut())
            .map_err(|_| anyhow!("Key derivation failed"))?;
        Ok(aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, bytes.as_ref())
                .map_err(|_| anyhow!("Key initialization failed"))?,
        ))
    }

    pub fn seal(
        &self,
        room: &str,
        sender: &str,
        recipient: &Peer,
        generation: u32,
        media: &MediaKey,
    ) -> Result<Envelope> {
        let me = self.peer(sender.to_owned());
        let context = context(room, &me, recipient, generation)?;
        let key = self.wrapping_key(recipient, &context)?;
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let mut ciphertext = Zeroizing::new(media.0.to_vec());
        key.seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(&context),
            &mut *ciphertext,
        )
        .map_err(|_| anyhow!("Key wrapping failed"))?;
        Ok(Envelope {
            sender: sender.to_owned(),
            recipient: recipient.identity.clone(),
            generation,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: URL_SAFE_NO_PAD.encode(&ciphertext),
        })
    }

    pub fn open(
        &self,
        room: &str,
        recipient: &str,
        sender: &Peer,
        envelope: &Envelope,
    ) -> Result<MediaKey> {
        ensure!(
            valid_envelope(envelope)
                && envelope.sender == sender.identity
                && envelope.recipient == recipient,
            "Invalid envelope participants"
        );
        let me = self.peer(recipient.to_owned());
        let context = context(room, sender, &me, envelope.generation)?;
        let key = self.wrapping_key(sender, &context)?;
        let nonce = decode::<12>(&envelope.nonce)?;
        let mut ciphertext = Zeroizing::new(decode::<48>(&envelope.ciphertext)?);
        let plaintext = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(&context),
                ciphertext.as_mut(),
            )
            .map_err(|_| anyhow!("Key envelope authentication failed"))?;
        let bytes: [u8; 32] = plaintext
            .try_into()
            .map_err(|_| anyhow!("Invalid media key"))?;
        Ok(MediaKey(Zeroizing::new(bytes)))
    }
}

fn context(room: &str, sender: &Peer, recipient: &Peer, generation: u32) -> Result<Vec<u8>> {
    room.parse::<crate::RoomCode>()?;
    ensure!(
        valid_identity(&sender.identity)
            && valid_identity(&recipient.identity)
            && sender.identity != recipient.identity
            && valid_public_key(&sender.public_key)
            && valid_public_key(&recipient.public_key),
        "Invalid exchange context"
    );
    Ok(serde_json::to_vec(&(
        "telinha-e2ee-v1",
        room,
        &sender.identity,
        &sender.public_key,
        &recipient.identity,
        &recipient.public_key,
        generation,
    ))?)
}

pub struct MediaKey(Zeroizing<[u8; 32]>);

impl Default for MediaKey {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaKey {
    pub fn new() -> Self {
        let mut bytes = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(bytes.as_mut());
        Self(bytes)
    }

    /// Both native and web providers use PBKDF2 over this canonical ASCII string.
    pub fn provider_material(&self) -> Zeroizing<String> {
        Zeroizing::new(URL_SAFE_NO_PAD.encode(self.0.as_ref()))
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Identity([private])")
    }
}
impl fmt::Debug for MediaKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MediaKey([private])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROOM: &str = "k7f2qxmn3dpa";

    #[test]
    fn webcrypto_vector_uses_the_same_ecdh_hkdf_aad_and_aes_gcm() {
        let vector: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/e2ee-v1.json")).unwrap();
        let secret =
            decode::<32>(vector["recipient"]["privateTestOnly"].as_str().unwrap()).unwrap();
        let bob = Identity(SecretKey::from_slice(&secret).unwrap());
        assert_eq!(
            bob.public_key(),
            vector["recipient"]["publicKey"].as_str().unwrap()
        );
        let sender = Peer {
            identity: "alice".into(),
            public_key: vector["sender"]["publicKey"].as_str().unwrap().into(),
            can_publish: true,
        };
        let envelope: Envelope = serde_json::from_value(vector["envelope"].clone()).unwrap();
        let result = bob.open(ROOM, "bob", &sender, &envelope).unwrap();
        assert_eq!(
            result.provider_material().as_str(),
            vector["mediaKeyTestOnly"].as_str().unwrap()
        );
    }

    #[test]
    fn only_the_named_recipient_can_unwrap_a_sender_key() {
        let alice = Identity::new();
        let bob = Identity::new();
        let mallory = Identity::new();
        let media = MediaKey::new();
        let envelope = alice
            .seal(ROOM, "alice", &bob.peer("bob".into()), 0, &media)
            .unwrap();
        let decoded = bob
            .open(ROOM, "bob", &alice.peer("alice".into()), &envelope)
            .unwrap();
        assert_eq!(*media.provider_material(), *decoded.provider_material());
        assert!(
            mallory
                .open(ROOM, "bob", &alice.peer("alice".into()), &envelope)
                .is_err()
        );
        assert!(
            bob.open(ROOM, "mallory", &alice.peer("alice".into()), &envelope)
                .is_err()
        );
        assert!(
            !serde_json::to_string(&envelope)
                .unwrap()
                .contains(media.provider_material().as_str())
        );
    }

    #[test]
    fn wrapping_authenticates_room_generation_identities_and_ciphertext() {
        let alice = Identity::new();
        let bob = Identity::new();
        let envelope = alice
            .seal(ROOM, "alice", &bob.peer("bob".into()), 7, &MediaKey::new())
            .unwrap();
        assert!(
            bob.open(
                "aaaaaaaaaaaa",
                "bob",
                &alice.peer("alice".into()),
                &envelope
            )
            .is_err()
        );
        let mut altered = envelope.clone();
        altered.generation += 1;
        assert!(
            bob.open(ROOM, "bob", &alice.peer("alice".into()), &altered)
                .is_err()
        );
        let mut altered = envelope.clone();
        altered.sender = "mallory".into();
        assert!(
            bob.open(ROOM, "bob", &alice.peer("mallory".into()), &altered)
                .is_err()
        );
        let mut altered = envelope.clone();
        let mut bytes = decode::<48>(&altered.ciphertext).unwrap();
        bytes[0] ^= 1;
        altered.ciphertext = URL_SAFE_NO_PAD.encode(bytes);
        assert!(
            bob.open(ROOM, "bob", &alice.peer("alice".into()), &altered)
                .is_err()
        );
    }

    #[test]
    fn invalid_public_keys_and_noncanonical_messages_are_rejected() {
        assert!(!valid_public_key(&URL_SAFE_NO_PAD.encode([0u8; 65])));
        assert!(!valid_public_key(&(Identity::new().public_key() + "=")));
        assert!(!valid_identity("name\nsecret"));
        assert!(!valid_identity(""));
        assert_eq!(format!("{:?}", MediaKey::new()), "MediaKey([private])");
    }
}
