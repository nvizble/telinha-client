use super::*;
use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use telinha_shared::e2ee::RelayCredentials;

struct RelayMock {
    session: Session,
    replies: Arc<Mutex<VecDeque<(StatusCode, Value)>>>,
    polls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RelayMock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RelayMock {
    async fn new(replies: Vec<(StatusCode, Value)>, first_delay: Duration) -> Self {
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let polls = Arc::new(AtomicUsize::new(0));
        let handler = {
            let replies = replies.clone();
            let polls = polls.clone();
            move || {
                let replies = replies.clone();
                let polls = polls.clone();
                async move {
                    let first = polls.fetch_add(1, Ordering::SeqCst) == 0;
                    let reply = {
                        let mut replies = replies.lock().unwrap();
                        if replies.len() > 1 {
                            replies.pop_front().unwrap()
                        } else {
                            replies.front().unwrap().clone()
                        }
                    };
                    if first {
                        tokio::time::sleep(first_delay).await;
                    }
                    (reply.0, Json(reply.1))
                }
            }
        };
        let app = Router::new()
            .route("/e2ee/state", get(handler))
            .route("/e2ee/keys", post(|| async { StatusCode::NO_CONTENT }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut credentials = SessionCredentials {
            url: "ws://localhost:7880".into(),
            token: "unused-test-media-token".into(),
            room: "k7f2qxmn3dpa".into(),
            identity: "alice".into(),
            expires_in: 3600,
            e2ee: Some(RelayCredentials {
                protocol: PROTOCOL,
                token: "a".repeat(64),
            }),
            feedback_token: None,
        };
        let session = Session::new(&url, &mut credentials, Identity::new()).unwrap();
        Self {
            session,
            replies,
            polls,
            task,
        }
    }
}

fn empty() -> Value {
    json!({"protocol": 1, "peers": [], "envelopes": []})
}

#[tokio::test]
async fn transient_http_failures_recover_but_revocation_stops_immediately() {
    let mut relay = RelayMock::new(
        vec![
            (StatusCode::SERVICE_UNAVAILABLE, json!({})),
            (StatusCode::OK, empty()),
            (StatusCode::TOO_MANY_REQUESTS, json!({})),
            (StatusCode::OK, empty()),
            (StatusCode::UNAUTHORIZED, json!({})),
        ],
        Duration::ZERO,
    )
    .await;
    let error = tokio::time::timeout(Duration::from_secs(10), relay.session.run())
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(relay.polls.load(Ordering::SeqCst), 5);
    assert_eq!(failure_category(&error), "authorization_revoked");
    assert!(!transient(&error));
}

#[tokio::test]
async fn a_timed_out_poll_is_retried_without_replacing_the_media_key() {
    let mut relay = RelayMock::new(
        vec![
            (StatusCode::OK, empty()),
            (StatusCode::OK, empty()),
            (StatusCode::UNAUTHORIZED, json!({})),
        ],
        Duration::from_millis(200),
    )
    .await;
    relay.session.client = reqwest::Client::builder()
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    let original = relay.session.media.provider_material();
    let error = tokio::time::timeout(Duration::from_secs(6), relay.session.run())
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(relay.polls.load(Ordering::SeqCst), 3);
    assert!(relay.session.media.provider_material() == original);
    assert_eq!(failure_category(&error), "authorization_revoked");
}

#[tokio::test]
async fn a_prolonged_relay_outage_cannot_bypass_the_membership_lease() {
    let mut relay = RelayMock::new(
        vec![(StatusCode::SERVICE_UNAVAILABLE, json!({}))],
        Duration::ZERO,
    )
    .await;
    relay.session.last_poll = Instant::now() - MAX_RELAY_GAP;
    assert!(relay.session.run().await.is_err());
    assert_eq!(relay.polls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn one_missing_peer_key_does_not_disconnect_the_session_or_install_a_fallback() {
    let bob = Identity::new();
    let state = json!({"protocol": 1, "peers": [bob.peer("bob".into())], "envelopes": []});
    let mut relay = RelayMock::new(vec![(StatusCode::OK, state)], Duration::ZERO).await;
    relay.session.exchange().await.unwrap();
    relay
        .session
        .pending
        .insert("bob".into(), Instant::now() - Duration::from_secs(31));
    relay.session.exchange().await.unwrap();
    assert!(relay.session.provider.get_key(&"bob".into(), 0).is_none());
    assert!(relay.session.pending_warned.contains("bob"));
    assert_eq!(relay.session.generation, 0);
}

#[tokio::test]
async fn leaving_still_rotates_keys_and_reappearing_identity_is_rejected() {
    let bob = Identity::new();
    let state = json!({"protocol": 1, "peers": [bob.peer("bob".into())], "envelopes": []});
    let mut relay = RelayMock::new(vec![(StatusCode::OK, state.clone())], Duration::ZERO).await;
    relay.session.exchange().await.unwrap();
    let original = relay.session.media.provider_material();
    *relay.replies.lock().unwrap() = VecDeque::from([(StatusCode::OK, empty())]);
    relay.session.exchange().await.unwrap();
    assert_eq!(relay.session.generation, 1);
    assert!(relay.session.media.provider_material() != original);
    *relay.replies.lock().unwrap() = VecDeque::from([(StatusCode::OK, state)]);
    let error = relay.session.run().await.unwrap_err();
    assert!(!transient(&error));
    assert_eq!(failure_category(&error), "invalid_encryption_state");
}

#[tokio::test]
async fn malformed_control_state_is_never_treated_as_a_network_retry() {
    let mut relay = RelayMock::new(
        vec![(
            StatusCode::OK,
            json!({"protocol": 999, "peers": [], "envelopes": []}),
        )],
        Duration::ZERO,
    )
    .await;
    let error = relay.session.run().await.unwrap_err();
    assert!(!transient(&error));
    assert_eq!(relay.polls.load(Ordering::SeqCst), 1);
}
