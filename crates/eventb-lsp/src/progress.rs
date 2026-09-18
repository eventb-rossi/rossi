//! `$/progress` reporting against a client-acknowledged token, degrading to
//! log messages when the client lacks `window.workDoneProgress`.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::lsp_types::*;
use tower_lsp_server::Client;

pub(crate) struct Progress {
    client: Client,
    token: Option<ProgressToken>,
    title: String,
}

impl Progress {
    pub(crate) async fn begin(client: &Client, supported: bool, title: &str) -> Self {
        static NEXT_TOKEN: AtomicU64 = AtomicU64::new(0);
        let mut token = None;
        if supported {
            let candidate = ProgressToken::String(format!(
                "rossi-progress-{}",
                NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
            ));
            let created = client
                .send_request::<request::WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                    token: candidate.clone(),
                })
                .await;
            if created.is_ok() {
                client
                    .send_notification::<notification::Progress>(ProgressParams {
                        token: candidate.clone(),
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                            WorkDoneProgressBegin {
                                title: title.to_string(),
                                cancellable: Some(false),
                                message: None,
                                percentage: None,
                            },
                        )),
                    })
                    .await;
                token = Some(candidate);
            }
        }
        Self {
            client: client.clone(),
            token,
            title: title.to_string(),
        }
    }

    pub(crate) async fn report(&self, message: &str) {
        match &self.token {
            Some(token) => {
                self.client
                    .send_notification::<notification::Progress>(ProgressParams {
                        token: token.clone(),
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::Report(
                            WorkDoneProgressReport {
                                cancellable: Some(false),
                                message: Some(message.to_string()),
                                percentage: None,
                            },
                        )),
                    })
                    .await;
            }
            None => {
                self.client
                    .log_message(MessageType::INFO, format!("{}: {message}", self.title))
                    .await;
            }
        }
    }

    /// End the progress and show the flow's outcome — the tail every exit
    /// path of a lens flow shares, so no path can forget to close the
    /// progress before messaging.
    pub(crate) async fn finish(self, kind: MessageType, message: String) {
        if let Some(token) = &self.token {
            self.client
                .send_notification::<notification::Progress>(ProgressParams {
                    token: token.clone(),
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(
                        WorkDoneProgressEnd { message: None },
                    )),
                })
                .await;
        }
        self.client.show_message(kind, message).await;
    }
}
