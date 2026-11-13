use std::time::Duration;
use telinha_shared::feedback::{FeedbackRequest, FeedbackResponse};
use tokio::sync::mpsc;
pub mod editor;
mod policy;
pub use policy::{Draft, Reason, Receipt, State, Status};

pub struct Submission {
    pub id: u64,
    pub request: FeedbackRequest,
}
pub struct Reply {
    pub id: u64,
    pub result: Result<(), String>,
}
pub struct Client {
    pub commands: mpsc::UnboundedSender<Submission>,
    pub events: async_channel::Receiver<Reply>,
}
pub struct Worker {
    endpoint: String,
    commands: mpsc::UnboundedReceiver<Submission>,
    events: async_channel::Sender<Reply>,
}
pub fn channel(server: &str) -> (Client, Worker) {
    let (commands, receiver) = mpsc::unbounded_channel();
    let (sender, events) = async_channel::unbounded();
    (
        Client { commands, events },
        Worker {
            endpoint: format!("{}/feedback", server.trim_end_matches('/')),
            commands: receiver,
            events: sender,
        },
    )
}
impl Worker {
    pub async fn run(mut self) {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build();
        let mut tasks = tokio::task::JoinSet::new();
        while let Some(submission) = self.commands.recv().await {
            while tasks.try_join_next().is_some() {}
            let sender = self.events.clone();
            let endpoint = self.endpoint.clone();
            let Ok(client) = client.as_ref().cloned() else {
                let _ = sender.try_send(Reply {
                    id: submission.id,
                    result: Err("Não foi possível preparar o envio. Tente novamente.".into()),
                });
                continue;
            };
            tasks.spawn(async move {
                let result = send(&client, &endpoint, &submission.request).await;
                let _ = sender
                    .send(Reply {
                        id: submission.id,
                        result,
                    })
                    .await;
            });
        }
        while tasks.join_next().await.is_some() {}
    }
}
async fn send(
    client: &reqwest::Client,
    endpoint: &str,
    request: &FeedbackRequest,
) -> Result<(), String> {
    let response = client
        .post(endpoint)
        .json(request)
        .send()
        .await
        .map_err(|_| {
            "Não foi possível confirmar o envio. Confira a conexão e tente novamente.".to_owned()
        })?;
    match response.status().as_u16() {
        200..=299 => {
            let response = response
                .json::<FeedbackResponse>()
                .await
                .map_err(|_| "A confirmação não chegou. Tente enviar novamente.".to_owned())?;
            if response.id.is_empty() {
                return Err("A confirmação não chegou. Tente enviar novamente.".into());
            }
            Ok(())
        }
        401 | 403 => Err(
            "O prazo para avaliar esta sessão terminou. Você pode fechar este formulário.".into(),
        ),
        429 => Err("Muitos envios agora. Aguarde um pouco e tente novamente.".into()),
        _ => Err(
            "Não foi possível enviar agora. Suas respostas continuam aqui para tentar novamente."
                .into(),
        ),
    }
}
