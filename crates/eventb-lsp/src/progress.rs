//! `$/progress` reporting against a client-acknowledged token, degrading to
//! log messages when the client lacks `window.workDoneProgress`, and the
//! cancellation the client can ask for through `window/workDoneProgress/cancel`.
//!
//! The framework has no hook for that notification, so the server registers
//! it as a custom method and routes it here: every progress in flight is
//! registered by token in a [`CancelRegistry`], a cancel request flips the
//! matching [`Cancel`], and the flow that owns the progress polls it between
//! steps or awaits it beside a blocking wait.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use tokio::sync::watch;
use tower_lsp_server::Client;

use crate::lsp_types::*;

/// The cancel state of one progress: set once, never reset.
///
/// A `watch` channel rather than a flag plus a notifier, so an awaiter that
/// subscribes after the cancel has already happened still returns at once.
pub struct Cancel {
    cancelled: watch::Sender<bool>,
}

impl Default for Cancel {
    fn default() -> Self {
        Self {
            cancelled: watch::channel(false).0,
        }
    }
}

impl Cancel {
    fn cancel(&self) {
        self.cancelled.send_replace(true);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }

    /// Resolves once the client cancelled, immediately if it already has.
    pub(crate) async fn cancelled(&self) {
        let mut rx = self.cancelled.subscribe();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

/// Every progress in flight, by the token the client knows it under.
#[derive(Default)]
pub struct CancelRegistry {
    by_token: DashMap<ProgressToken, Arc<Cancel>>,
}

impl CancelRegistry {
    /// Cancel the progress `token` names. Returns whether one was in flight:
    /// a token the server no longer tracks is a race with its own end, not
    /// an error, and is ignored.
    pub(crate) fn cancel(&self, token: &ProgressToken) -> bool {
        match self.by_token.get(token) {
            Some(cancel) => {
                cancel.cancel();
                true
            }
            None => false,
        }
    }
}

pub(crate) struct Progress {
    client: Client,
    token: Option<ProgressToken>,
    title: String,
    cancel: Arc<Cancel>,
    registry: Arc<CancelRegistry>,
}

impl Progress {
    pub(crate) async fn begin(
        client: &Client,
        supported: bool,
        title: &str,
        registry: &Arc<CancelRegistry>,
    ) -> Self {
        static NEXT_TOKEN: AtomicU64 = AtomicU64::new(0);
        let cancel = Arc::new(Cancel::default());
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
                // Registered before the Begin goes out, so a cancel that
                // arrives the instant the client sees the progress finds it.
                registry
                    .by_token
                    .insert(candidate.clone(), Arc::clone(&cancel));
                client
                    .send_notification::<notification::Progress>(ProgressParams {
                        token: candidate.clone(),
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                            WorkDoneProgressBegin {
                                title: title.to_string(),
                                cancellable: Some(true),
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
            cancel,
            registry: Arc::clone(registry),
        }
    }

    /// The cancel handle, for a wait that should end the moment the client
    /// asks rather than at its next step boundary.
    pub(crate) fn cancel(&self) -> &Arc<Cancel> {
        &self.cancel
    }

    /// Whether the client cancelled, for a flow checking between steps.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub(crate) async fn report(&self, message: &str) {
        match &self.token {
            Some(token) => {
                self.client
                    .send_notification::<notification::Progress>(ProgressParams {
                        token: token.clone(),
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::Report(
                            WorkDoneProgressReport {
                                cancellable: Some(true),
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
            self.registry.by_token.remove(token);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_cancel_is_seen_by_a_later_and_an_earlier_awaiter() {
        let cancel = Arc::new(Cancel::default());
        assert!(!cancel.is_cancelled());

        // Someone already waiting.
        let waiter = {
            let cancel = Arc::clone(&cancel);
            tokio::spawn(async move { cancel.cancelled().await })
        };
        cancel.cancel();
        waiter.await.unwrap();
        assert!(cancel.is_cancelled());

        // Someone who starts waiting only now returns at once.
        cancel.cancelled().await;
    }

    #[test]
    fn cancelling_an_unknown_token_is_a_no_op() {
        let registry = CancelRegistry::default();
        assert!(!registry.cancel(&ProgressToken::String("rossi-progress-99".into())));

        let cancel = Arc::new(Cancel::default());
        let token = ProgressToken::Number(7);
        registry.by_token.insert(token.clone(), Arc::clone(&cancel));
        assert!(registry.cancel(&token));
        assert!(cancel.is_cancelled());
    }
}
