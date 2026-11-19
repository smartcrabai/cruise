//! Copy-truncate rotation for the detached daemon's append-only log.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CHECK_INTERVAL: Duration = Duration::from_secs(60);
const ROTATION_THRESHOLD: u64 = 5 * 1024 * 1024;

pub(crate) fn start(path: PathBuf) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(CHECK_INTERVAL).await;
            if let Err(error) = rotate_if_needed(&path, ROTATION_THRESHOLD) {
                eprintln!("daemon log rotation failed: {error}");
            }
        }
    });
}

fn rotate_if_needed(path: &Path, threshold: u64) -> io::Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if metadata.len() <= threshold {
        return Ok(false);
    }

    let mut source = File::open(path)?;
    let rotated = path.with_file_name("daemon.log.1");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut destination = options.open(&rotated)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        destination.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    crate::platform::set_owner_only_acl(&rotated)?;
    let mut copied = 0;
    loop {
        source.seek(SeekFrom::Start(copied))?;
        copied += io::copy(&mut source, &mut destination)?;
        if copied >= source.metadata()?.len() {
            break;
        }
    }
    destination.flush()?;
    drop(destination);

    OpenOptions::new().write(true).open(path)?.set_len(0)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::rotate_if_needed;
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Read as _, Write as _};
    use std::sync::{Arc, Barrier, mpsc};
    use std::time::Duration;

    #[test]
    fn rotation_replaces_archive_and_truncates_live_log() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let log = directory.path().join("daemon.log");
        let archive = directory.path().join("daemon.log.1");
        fs::write(&log, "newest log contents")?;
        fs::write(&archive, "previous generation")?;

        assert!(rotate_if_needed(&log, 1)?);

        assert_eq!(fs::read_to_string(&archive)?, "newest log contents");
        assert_eq!(fs::metadata(&log)?.len(), 0);
        Ok(())
    }

    #[test]
    fn log_at_or_below_threshold_is_not_rotated() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let log = directory.path().join("daemon.log");
        let archive = directory.path().join("daemon.log.1");
        fs::write(&log, "12345")?;

        assert!(!rotate_if_needed(&log, 6)?);
        assert_eq!(fs::read_to_string(&log)?, "12345");
        assert!(!archive.exists());
        Ok(())
    }

    #[test]
    fn concurrent_appenders_keep_writing_after_rotation() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let log = directory.path().join("daemon.log");
        let archive = directory.path().join("daemon.log.1");
        fs::write(&log, vec![b'x'; 16 * 1024 * 1024])?;
        let append_log = OpenOptions::new().append(true).open(&log)?;
        let barrier = Arc::new(Barrier::new(2));

        std::thread::scope(|scope| -> io::Result<()> {
            let worker_barrier = Arc::clone(&barrier);
            let mut worker_append_log = &append_log;
            let (rotated_sender, rotated_receiver) = mpsc::channel();
            let worker = scope.spawn(move || -> io::Result<()> {
                worker_barrier.wait();
                for index in 0..64 {
                    writeln!(worker_append_log, "concurrent append {index}")?;
                    std::thread::sleep(Duration::from_millis(1));
                }
                rotated_receiver.recv().map_err(io::Error::other)?;
                writeln!(worker_append_log, "append after rotation")?;
                Ok(())
            });

            barrier.wait();
            assert!(rotate_if_needed(&log, 1024)?);
            rotated_sender.send(()).map_err(io::Error::other)?;
            worker
                .join()
                .map_err(|_| io::Error::other("append worker panicked"))??;
            Ok(())
        })?;

        let mut archived = File::open(&archive)?;
        assert!(archived.metadata()?.len() >= 16 * 1024 * 1024);
        let mut prefix = [0; 1];
        archived.read_exact(&mut prefix)?;
        assert_eq!(prefix[0], b'x');
        assert!(fs::read_to_string(&log)?.contains("append after rotation"));
        Ok(())
    }
}
