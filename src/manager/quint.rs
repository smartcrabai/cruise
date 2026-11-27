use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::artifacts;
use crate::error::{CruiseError, Result};
use crate::quint_workflow::{Files, MODEL_LIMIT, Preview, Run, Stage, sha256};
use crate::session::{SessionManager, SessionState};

pub(crate) const VARIABLE: &str = "quint.goal_approved";
const DIRECTORY: &str = "manager-quint";
const FILE: &str = "approved.qnt";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Binding {
    source_session: String,
    source_execution: String,
    model_sha256: String,
    png_sha256: String,
}

fn source<'a>(session: &'a SessionState, repository: &Path) -> Result<(&'a str, &'a Preview)> {
    if super::repository(&session.base_dir)? != repository {
        return Err(CruiseError::Other(
            "Quint source belongs to another repository".into(),
        ));
    }
    let Some(execution) = session.execution_id.as_deref() else {
        return Err(CruiseError::Other(
            "Quint source has no execution ID".into(),
        ));
    };
    let Some(
        Stage::Approved { preview }
        | Stage::Running { preview, .. }
        | Stage::MainFailed { preview }
        | Stage::Verified { preview, .. },
    ) = session.quint_run.as_ref().and_then(Run::stage)
    else {
        return Err(CruiseError::Other(
            "Quint source has no approved model".into(),
        ));
    };
    Ok((execution, preview))
}

impl Binding {
    fn pin(session: &SessionState, execution: &str, preview: &Preview) -> Self {
        Self {
            source_session: session.id.clone(),
            source_execution: execution.to_owned(),
            model_sha256: preview.model_sha256.clone(),
            png_sha256: preview.image.sha256.clone(),
        }
    }

    pub(super) fn capture(
        store: &SessionManager,
        repository: &Path,
        session: &str,
    ) -> Result<Self> {
        let saved = store.load(session)?;
        let (execution, preview) = source(&saved, repository)?;
        let pinned = Self::pin(&saved, execution, preview);
        pinned.read(store, repository)?;
        Ok(pinned)
    }

    fn matches(&self, session: &SessionState, execution: &str, preview: &Preview) -> bool {
        self.source_session == session.id
            && self.source_execution == execution
            && self.model_sha256 == preview.model_sha256
            && self.png_sha256 == preview.image.sha256
    }

    pub(crate) fn read(
        &self,
        store: &SessionManager,
        repository: &Path,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let session = store.load(&self.source_session)?;
        let (execution, preview) = source(&session, repository)?;
        if !self.matches(&session, execution, preview) {
            return Err(CruiseError::Other(
                "Quint source approval changed; review the current source execution".into(),
            ));
        }
        let root = session.artifacts_path(&store.sessions_dir());
        let files = Files::new(&root, Some(execution))?;
        if matches!(
            session.quint_run.as_ref().and_then(Run::stage),
            Some(Stage::Approved { .. })
        ) && !files.rendered_candidate(preview)?
        {
            return Err(CruiseError::Other(
                "Quint candidate changed after review".into(),
            ));
        }
        let model = files.approved_bytes(preview)?;
        let png = preview.image.read(&root)?;
        let latest = store.load(&self.source_session)?;
        let (execution, preview) = source(&latest, repository)?;
        if !self.matches(&latest, execution, preview) {
            return Err(CruiseError::Other(
                "Quint source approval changed during verification".into(),
            ));
        }
        Ok((model, png))
    }

    pub(crate) fn write_child(
        &self,
        store: &SessionManager,
        child: &SessionState,
        model: &[u8],
    ) -> Result<()> {
        let directory = directory(&store.sessions_dir(), &child.id);
        artifacts::ensure_private_directory(&directory)?;
        match std::fs::symlink_metadata(directory.join(FILE)) {
            Ok(_) => self.check_copy(&directory),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                artifacts::write_bytes(&directory, FILE, model, MODEL_LIMIT)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn verified_child_file(
        &self,
        store: &SessionManager,
        child: &SessionState,
    ) -> Result<PathBuf> {
        let repository = super::repository(&child.base_dir)?;
        self.read(store, &repository)?;
        let directory = directory(&store.sessions_dir(), &child.id);
        self.check_copy(&directory)?;
        Ok(directory.join(FILE))
    }

    fn check_copy(&self, directory: &Path) -> Result<()> {
        if sha256(&artifacts::read_bytes(directory, FILE, MODEL_LIMIT)?) != self.model_sha256 {
            return Err(CruiseError::Other(
                "Manager child Quint file changed".into(),
            ));
        }
        Ok(())
    }
}

fn directory(sessions: &Path, child_id: &str) -> PathBuf {
    sessions
        .join(child_id)
        .join(artifacts::ARTIFACTS_DIR)
        .join(DIRECTORY)
}

pub(crate) fn child_path(sessions: &Path, child_id: &str) -> PathBuf {
    directory(sessions, child_id).join(FILE)
}
