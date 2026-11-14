//! Automatic media-key exchange; no plaintext key is sent to the relay.
use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use livekit::{
    RoomOptions,
    e2ee::{
        E2eeOptions, EncryptionType,
        key_provider::{KeyProvider, KeyProviderOptions},
    },
};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use telinha_shared::{
    SessionCredentials,
    e2ee::{
        Identity, MAX_PEERS, MediaKey, PROTOCOL, Peer, RelayState, valid_identity, valid_public_key,
    },
};

// Stop before the relay's 30-second membership lease expires. Short HTTP
// failures must not tear down healthy encrypted WebRTC media.
const MAX_RELAY_GAP: Duration = Duration::from_secs(20);

fn transient(error: &anyhow::Error) -> bool {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<reqwest::Error>())
        .is_some_and(|error| match error.status() {
            Some(status) => status.is_server_error() || matches!(status.as_u16(), 408 | 429),
            None => !error.is_builder() && !error.is_redirect() && !error.is_decode(),
        })
}

fn failure_category(error: &anyhow::Error) -> &'static str {
    if let Some(error) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<reqwest::Error>())
    {
        match error.status().map(|status| status.as_u16()) {
            Some(401 | 403) => "authorization_revoked",
            Some(429) => "rate_limited",
            Some(_) => "http_error",
            None if error.is_timeout() => "timeout",
            None => "transport_error",
        }
    } else {
        "invalid_encryption_state"
    }
}

pub struct Session {
    identity: Identity,
    me: Peer,
    room: String,
    media: MediaKey,
    generation: u32,
    provider: KeyProvider,
    peers: HashMap<String, Peer>,
    sent: HashMap<String, u32>,
    received: HashMap<String, u32>,
    pending: HashMap<String, Instant>,
    pending_warned: HashSet<String>,
    retired: HashSet<String>,
    client: reqwest::Client,
    endpoint: String,
    token: String,
    last_poll: Instant,
}

impl Session {
    pub fn new(
        server: &str,
        credentials: &mut SessionCredentials,
        identity: Identity,
    ) -> Result<Self> {
        let relay = credentials
            .e2ee
            .take()
            .context("O servidor não ofereceu uma sessão criptografada.")?;
        ensure!(
            relay.protocol == PROTOCOL
                && relay.token.len() == 64
                && valid_identity(&credentials.identity),
            "O servidor enviou dados de criptografia incompatíveis."
        );
        let provider = KeyProvider::new(KeyProviderOptions {
            ratchet_window_size: 0,
            failure_tolerance: -1,
            ..Default::default()
        });
        let media = MediaKey::new();
        let me = identity.peer(credentials.identity.clone());
        ensure!(
            provider.set_key(
                &me.identity.clone().into(),
                0,
                media.provider_material().as_bytes().to_vec()
            ),
            "Não foi possível preparar a criptografia de mídia."
        );
        Ok(Self {
            identity,
            me,
            room: credentials.room.clone(),
            media,
            generation: 0,
            provider,
            peers: HashMap::new(),
            sent: HashMap::new(),
            received: HashMap::new(),
            pending: HashMap::new(),
            pending_warned: HashSet::new(),
            retired: HashSet::new(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(6))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            endpoint: format!("{}/e2ee", server.trim_end_matches('/')),
            token: relay.token,
            last_poll: Instant::now(),
        })
    }

    pub fn options(&self) -> RoomOptions {
        let mut options = RoomOptions::default();
        options.encryption = Some(E2eeOptions {
            encryption_type: EncryptionType::Gcm,
            key_provider: self.provider.clone(),
        });
        options
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut failures = 0u32;
        loop {
            let delay = match self.exchange().await {
                Ok(()) => {
                    if failures > 0 {
                        tracing::info!(
                            event = "e2ee_recovered",
                            attempts = failures,
                            "Troca de chaves recuperada"
                        );
                    }
                    failures = 0;
                    Duration::from_secs(1)
                }
                Err(error) if transient(&error) && self.last_poll.elapsed() < MAX_RELAY_GAP => {
                    failures = failures.saturating_add(1);
                    tracing::warn!(
                        event = "e2ee_retry",
                        reason = failure_category(&error),
                        attempt = failures,
                        "Falha transitória na troca de chaves; tentando novamente"
                    );
                    Duration::from_secs(1 << (failures - 1).min(2))
                        .min(MAX_RELAY_GAP.saturating_sub(self.last_poll.elapsed()))
                }
                Err(error) => {
                    tracing::warn!(
                        event = "e2ee_stopped",
                        reason = failure_category(&error),
                        relay_gap_seconds = self.last_poll.elapsed().as_secs(),
                        "A troca de chaves não pôde ser mantida"
                    );
                    return Err(error).context("A troca segura de chaves foi interrompida.");
                }
            };
            tokio::time::sleep(delay).await;
        }
    }

    pub async fn leave(&self) {
        let _ = self
            .client
            .delete(format!("{}/state", self.endpoint))
            .bearer_auth(&self.token)
            .timeout(Duration::from_millis(750))
            .send()
            .await;
    }

    async fn exchange(&mut self) -> Result<()> {
        let mut response = self
            .client
            .get(format!("{}/state", self.endpoint))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 128 * 1024,
                "Invalid key relay response"
            );
            bytes.extend_from_slice(&chunk);
        }
        let state: RelayState = serde_json::from_slice(&bytes)?;
        ensure!(
            state.protocol == PROTOCOL
                && state.peers.len() < MAX_PEERS
                && state.envelopes.len() < MAX_PEERS,
            "Incompatible key relay response"
        );
        self.last_poll = Instant::now();
        let mut peers = HashMap::new();
        for peer in state.peers {
            ensure!(
                valid_identity(&peer.identity)
                    && valid_public_key(&peer.public_key)
                    && peer.identity != self.me.identity
                    && !self.retired.contains(&peer.identity),
                "Invalid peer identity"
            );
            if let Some(previous) = self.peers.get(&peer.identity) {
                ensure!(
                    previous == &peer,
                    "A participant's public key changed during the session"
                );
            }
            if peer.can_publish && !self.received.contains_key(&peer.identity) {
                self.pending
                    .entry(peer.identity.clone())
                    .or_insert_with(Instant::now);
            }
            ensure!(
                peers.insert(peer.identity.clone(), peer).is_none(),
                "Duplicate peer identity"
            );
        }
        let removed: Vec<_> = self
            .peers
            .keys()
            .filter(|id| !peers.contains_key(*id))
            .cloned()
            .collect();
        if !removed.is_empty() {
            self.generation = self
                .generation
                .checked_add(1)
                .context("Key generation exhausted")?;
            self.media = MediaKey::new();
            ensure!(
                self.provider.set_key(
                    &self.me.identity.clone().into(),
                    0,
                    self.media.provider_material().as_bytes().to_vec()
                ),
                "Media key rotation failed"
            );
            for identity in removed {
                self.retired.insert(identity.clone());
                self.received.remove(&identity);
                self.provider.set_key(
                    &identity.into(),
                    0,
                    MediaKey::new().provider_material().as_bytes().to_vec(),
                );
            }
            ensure!(
                self.retired.len() <= 4096,
                "Reconnect to renew encryption state"
            );
            self.sent.clear();
        }
        self.sent.retain(|id, _| peers.contains_key(id));
        self.pending.retain(|id, _| peers.contains_key(id));
        self.pending_warned.retain(|id| peers.contains_key(id));
        self.peers = peers;
        for envelope in state.envelopes {
            let Some(sender) = self.peers.get(&envelope.sender) else {
                continue;
            };
            if self
                .received
                .get(&envelope.sender)
                .is_some_and(|generation| *generation >= envelope.generation)
            {
                continue;
            }
            let key = self
                .identity
                .open(&self.room, &self.me.identity, sender, &envelope)?;
            ensure!(
                self.provider.set_key(
                    &sender.identity.clone().into(),
                    0,
                    key.provider_material().as_bytes().to_vec()
                ),
                "Media key installation failed"
            );
            self.pending.remove(&envelope.sender);
            self.pending_warned.remove(&envelope.sender);
            self.received.insert(envelope.sender, envelope.generation);
        }
        let delayed: Vec<_> = self
            .pending
            .iter()
            .filter(|(id, since)| {
                since.elapsed() >= Duration::from_secs(30) && !self.pending_warned.contains(*id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        if !delayed.is_empty() {
            // Missing keys block only that publisher's encrypted media. One
            // stalled participant must not disconnect everybody else.
            tracing::warn!(
                event = "e2ee_peer_key_delayed",
                peers = delayed.len(),
                "Aguardando chaves de participantes"
            );
            self.pending_warned.extend(delayed);
        }
        let mut outgoing = Vec::new();
        for recipient in self.peers.values() {
            if self.sent.get(&recipient.identity) == Some(&self.generation) {
                continue;
            }
            let envelope = self.identity.seal(
                &self.room,
                &self.me.identity,
                recipient,
                self.generation,
                &self.media,
            )?;
            outgoing.push((recipient.identity.clone(), envelope));
        }
        let endpoint = format!("{}/keys", self.endpoint);
        let client = &self.client;
        let token = &self.token;
        let mut sends = futures_util::stream::iter(outgoing)
            .map(|(identity, envelope)| {
                let endpoint = &endpoint;
                async move {
                    let response = client
                        .post(endpoint)
                        .bearer_auth(token)
                        .json(&envelope)
                        .send()
                        .await?;
                    let delivered = response.status() != reqwest::StatusCode::NOT_FOUND;
                    if delivered {
                        response.error_for_status()?;
                    }
                    Ok::<_, reqwest::Error>((identity, delivered))
                }
            })
            .buffer_unordered(8);
        while let Some(result) = sends.next().await {
            let (identity, delivered) = result?;
            if delivered {
                self.sent.insert(identity, self.generation);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
