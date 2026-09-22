//! Spawning long-running application operations.
//!
//! The browser learns about progress and completion from the SSE stream, so
//! action handlers return immediately and never await the operation.

use crate::error::CruiseError;
use crate::session::SessionState;

/// Run `future` detached, reporting non-cancellation failures on stderr.
pub(crate) fn spawn<F>(future: F)
where
    F: Future<Output = crate::error::Result<SessionState>> + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(error) = future.await
            && !matches!(error, CruiseError::Interrupted)
        {
            eprintln!("webui operation failed: {error}");
        }
    });
}
