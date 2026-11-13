use anyhow::{Context, bail, ensure};
use parking_lot::Mutex;
use std::{
    io::Cursor,
    sync::Arc,
    time::{Duration, Instant},
};
use telinha_shared::{CallStatus, LoginStart, LoginStatus, UserProfile};
use tokio::sync::mpsc;

pub enum Command {
    Begin(u64),
    Restore(u64, String),
    Cancel(u64),
    Logout(u64),
    Avatar(String),
    Discover(u64),
}
pub enum Event {
    Browser(String),
    Ready {
        user: UserProfile,
        save_token: Option<String>,
    },
    SignedOut {
        clear_saved: bool,
    },
    Error(String),
    Call(CallStatus),
    CallError {
        message: String,
        retry_in: u64,
    },
    Avatar {
        url: String,
        width: u32,
        height: u32,
        bgra: Vec<u8>,
    },
}
pub struct Envelope {
    pub generation: Option<u64>,
    pub event: Event,
}
pub struct Account {
    pub commands: mpsc::UnboundedSender<Command>,
    pub events: async_channel::Receiver<Envelope>,
    pub token: Arc<Mutex<Option<String>>>,
    pub credential_key: String,
    pub invite_url: String,
}
pub struct Worker {
    server: String,
    commands: mpsc::UnboundedReceiver<Command>,
    events: async_channel::Sender<Envelope>,
    token: Arc<Mutex<Option<String>>>,
}

pub fn channel(server: &str) -> (Account, Worker) {
    let (commands, receiver) = mpsc::unbounded_channel();
    let (sender, events) = async_channel::unbounded();
    let token = Arc::new(Mutex::new(None));
    (
        Account {
            commands,
            events,
            token: token.clone(),
            credential_key: format!("telinha-discord:{}", server.trim_end_matches('/')),
            invite_url: format!("{}/discord/invite", server.trim_end_matches('/')),
        },
        Worker {
            server: server.trim_end_matches('/').into(),
            commands: receiver,
            events: sender,
            token,
        },
    )
}
fn send(events: &async_channel::Sender<Envelope>, generation: u64, event: Event) {
    let _ = events.try_send(Envelope {
        generation: Some(generation),
        event,
    });
}

// GPUI 0.2.2 propagates CredReadW/CredDeleteW's ERROR_NOT_FOUND on Windows,
// including the normal first launch and deletion of an already absent entry.
pub fn credentials_missing(error: &anyhow::Error) -> bool {
    #[cfg(target_os = "windows")]
    {
        error
            .downcast_ref::<windows::core::Error>()
            .is_some_and(|error| {
                error.code()
                    == windows::core::HRESULT::from_win32(
                        windows::Win32::Foundation::ERROR_NOT_FOUND.0,
                    )
            })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = error;
        false
    }
}

async fn checked(response: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    #[derive(serde::Deserialize)]
    struct ErrorBody {
        message: String,
    }
    let message = response
        .json::<ErrorBody>()
        .await
        .ok()
        .map(|body| body.message)
        .unwrap_or_else(|| "O servidor não conseguiu concluir o login. Tente novamente.".into());
    bail!(message)
}

pub fn avatar_url_is_valid(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("cdn.discordapp.com")
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && (url.path().starts_with("/avatars/") || url.path().starts_with("/embed/avatars/"))
    })
}

async fn load_avatar(http: reqwest::Client, url: String) -> anyhow::Result<Event> {
    ensure!(avatar_url_is_valid(&url), "Avatar inválido");
    let mut response = http.get(&url).send().await?.error_for_status()?;
    let mut bytes = vec![];
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 512 * 1024,
            "Avatar grande demais"
        );
        bytes.extend_from_slice(&chunk);
    }
    let (w, h) = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()?
        .into_dimensions()?;
    ensure!(w <= 512 && h <= 512, "Avatar grande demais");
    let image = image::load_from_memory(&bytes)?
        .thumbnail(64, 64)
        .to_rgba8();
    let (width, height) = image.dimensions();
    let mut bgra = image.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(Event::Avatar {
        url,
        width,
        height,
        bgra,
    })
}

impl Worker {
    pub async fn run(mut self) {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("cliente de conta");
        let server_valid = reqwest::Url::parse(&self.server).is_ok_and(|url| {
            url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
        });
        let pending = Arc::new(Mutex::new(None::<String>));
        let mut current: Option<tokio::task::JoinHandle<()>> = None;
        let mut background = tokio::task::JoinSet::new();
        while let Some(command) = self.commands.recv().await {
            while background.try_join_next().is_some() {}
            if let Command::Discover(generation) = command {
                let saved = self.token.lock().clone();
                let (http, server, events) =
                    (http.clone(), self.server.clone(), self.events.clone());
                background.spawn(async move {
                    let mut retry_in = 10;
                    let result: anyhow::Result<CallStatus> = async {
                        ensure!(server_valid, "O servidor precisa usar HTTPS.");
                        let saved = saved.context("Entre com Discord para encontrar sua call.")?;
                        let response = http
                            .get(format!("{server}/discord/call"))
                            .bearer_auth(saved)
                            .send()
                            .await?;
                        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                            retry_in = response
                                .headers()
                                .get(reqwest::header::RETRY_AFTER)
                                .and_then(|value| value.to_str().ok())
                                .and_then(|value| value.parse().ok())
                                .unwrap_or(60)
                                .clamp(3, 120);
                        }
                        Ok(checked(response).await?.json().await?)
                    }
                    .await;
                    let event = match result {
                        Ok(status) => Event::Call(status),
                        Err(_) => Event::CallError {
                            message: "Não foi possível conferir sua call. Tentando novamente…"
                                .into(),
                            retry_in,
                        },
                    };
                    send(&events, generation, event);
                });
                continue;
            }
            if let Command::Avatar(url) = command {
                let (http, events) = (http.clone(), self.events.clone());
                background.spawn(async move {
                    if let Ok(event) = load_avatar(http, url).await {
                        let _ = events.try_send(Envelope {
                            generation: None,
                            event,
                        });
                    }
                });
                continue;
            }
            if let Some(task) = current.take() {
                task.abort();
                let _ = task.await;
            }
            let previous = pending.lock().take();
            if let Some(value) = previous {
                let (http, server) = (http.clone(), self.server.clone());
                background.spawn(async move {
                    let _ = http
                        .post(format!("{server}/auth/discord/cancel"))
                        .bearer_auth(value)
                        .send()
                        .await;
                });
            }
            let (http, server, events, token, pending) = (
                http.clone(),
                self.server.clone(),
                self.events.clone(),
                self.token.clone(),
                pending.clone(),
            );
            current = Some(tokio::spawn(async move {
                let generation = match &command {
                    Command::Begin(g)
                    | Command::Restore(g, _)
                    | Command::Cancel(g)
                    | Command::Logout(g) => *g,
                    Command::Avatar(_) | Command::Discover(_) => unreachable!(),
                };
                if !server_valid {
                    send(
                        &events,
                        generation,
                        Event::Error("O login requer um servidor HTTPS.".into()),
                    );
                    return;
                }
                let result: anyhow::Result<()> = async {
                    match command {
                        Command::Begin(_) => {
                            let begin: LoginStart = checked(http.post(format!("{server}/auth/discord/start")).send().await?).await?.json().await?;
                            let browser = reqwest::Url::parse(&begin.browser_url)?;
                            ensure!(browser.origin() == reqwest::Url::parse(&server)?.origin() && browser.path() == "/auth/discord/open", "O servidor enviou um endereço de login inválido.");
                            *pending.lock() = Some(begin.poll_token.clone());
                            send(&events, generation, Event::Browser(begin.browser_url));
                            let until = Instant::now() + Duration::from_secs(begin.expires_in.min(600));
                            while Instant::now() < until {
                                tokio::time::sleep(Duration::from_secs(2)).await;
                                let status: LoginStatus = checked(http.get(format!("{server}/auth/discord/poll")).bearer_auth(&begin.poll_token).send().await?).await?.json().await?;
                                match status {
                                    LoginStatus::Pending => {},
                                    LoginStatus::Failed { message } => bail!(message),
                                    LoginStatus::Complete { session } => {
                                        *token.lock() = Some(session.token.clone()); pending.lock().take();
                                        send(&events, generation, Event::Ready { user: session.user, save_token: Some(session.token) }); return Ok(());
                                    }
                                }
                            }
                            bail!("O login expirou. Clique em Entrar com Discord para tentar novamente.");
                        }
                        Command::Restore(_, saved) => {
                            let response = http.get(format!("{server}/auth/me")).bearer_auth(&saved).send().await?;
                            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                                *token.lock() = None; send(&events, generation, Event::SignedOut { clear_saved: true });
                            } else {
                                let user = checked(response).await?.json::<UserProfile>().await?;
                                *token.lock() = Some(saved); send(&events, generation, Event::Ready { user, save_token: None });
                            }
                        }
                        Command::Cancel(_) => {
                            token.lock().take();
                            send(&events, generation, Event::SignedOut { clear_saved: false });
                        }
                        Command::Logout(_) => {
                            let saved = token.lock().take();
                            send(&events, generation, Event::SignedOut { clear_saved: true });
                            if let Some(saved) = saved {
                                checked(http.post(format!("{server}/auth/logout")).bearer_auth(saved).send().await.context("Você saiu neste dispositivo, mas não foi possível revogar a sessão no servidor.")?).await?;
                            }
                        }
                        Command::Avatar(_) | Command::Discover(_) => unreachable!(),
                    }
                    Ok(())
                }.await;
                if let Err(error) = result {
                    send(
                        &events,
                        generation,
                        Event::Error(format!("Não deu para concluir o login. {error}")),
                    );
                }
            }));
        }
        if let Some(task) = current {
            task.abort();
        }
    }
}
