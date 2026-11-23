//! Test fixture for a real Ask suspended on the managed application runtime.

use super::{ApplicationEvent, ApplicationRuntime, OperationKind, RuntimeAskHandler};
use crate::ask_handler::AskHandler as _;
use crate::cancellation::CancellationToken;
use crate::error::Result;
use crate::session::ManagedChildOwner;
use std::sync::{Arc, Mutex};
use tokio::{sync::oneshot, task::JoinHandle};

pub(crate) struct ManagedAsk {
    pub(crate) request_id: oneshot::Receiver<String>,
    pub(crate) result: JoinHandle<Result<String>>,
    pub(crate) cancel: CancellationToken,
}

pub(crate) fn start(
    runtime: &Arc<ApplicationRuntime>,
    session_id: &str,
    owner: &ManagedChildOwner,
) -> Result<ManagedAsk> {
    let claim = runtime.try_begin_managed(session_id, owner, OperationKind::Run)?;
    let cancel = claim.token();
    let (send, request_id) = oneshot::channel();
    let send = Mutex::new(Some(send));
    let handler = RuntimeAskHandler {
        runtime: Arc::clone(runtime),
        session_id: session_id.to_owned(),
        claim_identity: claim.identity(),
        token: cancel.clone(),
        sink: Arc::new(move |event| {
            if let ApplicationEvent::AskUserRequired { request_id, .. } = event
                && let Some(send) = send
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
            {
                let _ = send.send(request_id);
            }
            Ok(())
        }),
    };
    let result = tokio::task::spawn_blocking(move || {
        let _claim = claim;
        handler.ask_user("Which local answer should the managed task use?")
    });
    Ok(ManagedAsk {
        request_id,
        result,
        cancel,
    })
}
