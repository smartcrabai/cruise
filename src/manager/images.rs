//! Repository-bound image references. Neither client nor model supplies a daemon path.
use std::collections::{BTreeMap, HashSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use axum::body::Bytes;
use serde::{Deserialize, Serialize};

use super::{Snapshot, Task, hex_digest};
use crate::artifacts::ensure_private_directory;
use crate::error::{CruiseError, Result};

pub(crate) const LIMIT: usize = 1024 * 1024;
pub(crate) const MAX_FILES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Image {
    pub id: String,
    pub extension: String,
    pub sha256: String,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Upload {
    extension: String,
    #[serde(with = "encoded_bytes")]
    data: Bytes,
}

mod encoded_bytes {
    use axum::body::Bytes;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde::{Deserialize as _, Deserializer, Serializer, de::Error as _};

    pub(super) fn serialize<S: Serializer>(
        bytes: &Bytes,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&base64::display::Base64Display::new(bytes, &STANDARD))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Bytes, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        if encoded.len() > super::LIMIT.div_ceil(3) * 4 {
            return Err(D::Error::custom("Manager images exceed 1 MiB"));
        }
        STANDARD
            .decode(encoded)
            .map(Bytes::from)
            .map_err(D::Error::custom)
    }
}

fn invalid(message: &str) -> CruiseError {
    CruiseError::Other(message.into())
}

fn media_type(extension: &str) -> Result<&'static str> {
    match extension {
        "png" => Ok("image/png"),
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "gif" => Ok("image/gif"),
        "webp" => Ok("image/webp"),
        _ => Err(invalid("Unsupported Manager image type")),
    }
}

impl Upload {
    pub(crate) fn from_bytes(name: &Path, bytes: Bytes) -> Result<Self> {
        let extension = name
            .extension()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid("Manager image requires a supported extension"))?
            .to_ascii_lowercase();
        if bytes.len() > LIMIT {
            return Err(invalid("Manager images exceed 1 MiB"));
        }
        crate::issue_images::validate_image(media_type(&extension)?, &bytes)?;
        Ok(Self {
            extension,
            data: bytes,
        })
    }
}

pub(crate) fn read_uploads(paths: &[PathBuf]) -> Result<Vec<Upload>> {
    if paths.len() > MAX_FILES {
        return Err(invalid("Manager accepts at most eight images per message"));
    }
    let mut remaining = LIMIT;
    let mut uploads = Vec::with_capacity(paths.len());
    for path in paths {
        if !std::fs::metadata(path)?.is_file() {
            return Err(invalid("Manager image must be a regular file"));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(remaining as u64 + 1)
            .read_to_end(&mut bytes)?;
        remaining = remaining
            .checked_sub(bytes.len())
            .ok_or_else(|| invalid("Manager images exceed 1 MiB"))?;
        uploads.push(Upload::from_bytes(path, bytes.into())?);
    }
    Ok(uploads)
}

fn root(repo: &Path) -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?
        .join("manager")
        .join("images")
        .join(hex_digest(repo.to_string_lossy().as_bytes())))
}

pub(crate) fn store(repo: &Path, uploads: Vec<Upload>) -> Result<Vec<Image>> {
    if uploads.is_empty() {
        return Ok(Vec::new());
    }
    if uploads.len() > MAX_FILES {
        return Err(invalid("Manager accepts at most eight images per message"));
    }
    let root = root(repo)?;
    let mut total = 0;
    let mut decoded = Vec::with_capacity(uploads.len());
    for upload in uploads {
        let bytes = upload.data;
        total += bytes.len();
        if total > LIMIT {
            return Err(invalid("Manager images exceed 1 MiB"));
        }
        let extension =
            crate::issue_images::validate_image(media_type(&upload.extension)?, &bytes)?;
        decoded.push((
            Image {
                id: uuid::Uuid::new_v4().to_string(),
                extension: extension.into(),
                sha256: hex_digest(&bytes),
            },
            bytes,
        ));
    }
    ensure_private_directory(&root)?;
    decoded
        .into_iter()
        .map(|(image, bytes)| {
            crate::artifacts::write_bytes(&root, &image.filename()?, &bytes, LIMIT)?;
            Ok(image)
        })
        .collect()
}

impl Image {
    pub(crate) fn filename(&self) -> Result<String> {
        let id =
            uuid::Uuid::parse_str(&self.id).map_err(|_| invalid("Invalid Manager image ID"))?;
        media_type(&self.extension)?;
        Ok(format!("{id}.{}", self.extension))
    }

    pub(crate) fn read(&self, repo: &Path) -> Result<(&'static str, Vec<u8>)> {
        let bytes = crate::artifacts::read_bytes(&root(repo)?, &self.filename()?, LIMIT)?;
        self.verify(&bytes)?;
        Ok((media_type(&self.extension)?, bytes))
    }

    fn verify(&self, bytes: &[u8]) -> Result<()> {
        crate::issue_images::validate_image(media_type(&self.extension)?, bytes)?;
        if hex_digest(bytes) != self.sha256 {
            return Err(invalid("Manager image digest changed"));
        }
        Ok(())
    }
}

pub(crate) type TaskImages = BTreeMap<String, Vec<Image>>;
pub(crate) type RequestedImages = BTreeMap<String, Vec<String>>;

pub(super) fn bind(
    snapshot: &Snapshot,
    tasks: &[Task],
    requested: RequestedImages,
) -> Result<TaskImages> {
    requested.into_iter().map(|(task, ids)| {
        if !tasks.iter().any(|candidate| candidate.id == task) || ids.len() > MAX_FILES || ids.is_empty() {
            return Err(invalid("Invalid Manager task image assignment"));
        }
        let mut unique = HashSet::new();
        let images: Result<Vec<_>> = ids.iter().map(|id| {
            if !unique.insert(id) {
                return Err(invalid("Duplicate Manager task image assignment"));
            }
            let image = snapshot.transcript.iter().filter(|turn| turn.role == "user" && turn.status == "completed")
                .flat_map(|turn| &turn.images).find(|image| &image.id == id)
                .ok_or_else(|| invalid("Manager task image is not from a completed turn in this repository"))?;
            image.read(&snapshot.repository)?;
            Ok(image.clone())
        }).collect();
        Ok((task, images?))
    }).collect()
}

pub(crate) fn child_images<'a>(goal: &'a super::Goal, task_id: &str) -> &'a [Image] {
    goal.proposal
        .contract
        .as_ref()
        .and_then(|contract| contract.task_images.get(task_id))
        .map_or(&[], Vec::as_slice)
}

pub(crate) fn child_paths(target: &Path, images: &[Image]) -> Result<Vec<PathBuf>> {
    images
        .iter()
        .map(|image| Ok(target.join(image.filename()?)))
        .collect()
}

pub(crate) fn verify_child(repo: &Path, target: &Path, images: &[Image]) -> Result<()> {
    if images.is_empty() && !target.exists() {
        return Ok(());
    }
    ensure_private_directory(target)?;
    if std::fs::read_dir(target)?.count() != images.len() {
        return Err(invalid(
            "Manager child image inventory differs from approval",
        ));
    }
    for image in images {
        image.read(repo)?;
        image.verify(&crate::artifacts::read_bytes(
            target,
            &image.filename()?,
            LIMIT,
        )?)?;
    }
    Ok(())
}

pub(crate) fn copy(repo: &Path, target: &Path, images: &[Image]) -> Result<Vec<PathBuf>> {
    if images.is_empty() {
        return Ok(Vec::new());
    }
    ensure_private_directory(target)?;
    images
        .iter()
        .map(|image| {
            let (_, bytes) = image.read(repo)?;
            let name = image.filename()?;
            let path = target.join(&name);
            match std::fs::symlink_metadata(&path) {
                Ok(_) => image.verify(&crate::artifacts::read_bytes(target, &name, LIMIT)?)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    crate::artifacts::write_bytes(target, &name, &bytes, LIMIT)?;
                }
                Err(error) => return Err(error.into()),
            }
            Ok(path)
        })
        .collect()
}
