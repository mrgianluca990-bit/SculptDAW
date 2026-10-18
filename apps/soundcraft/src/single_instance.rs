//! macOS document launches share one process, one transport and one session.
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Instance {
    _lock: File,
    socket: UnixDatagram,
    path: PathBuf,
}

impl Instance {
    /// None means the request was delivered to the already-running workspace.
    pub fn acquire(directory: &Path, files: &[String]) -> io::Result<Option<Self>> {
        fs::create_dir_all(directory)?;
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(directory.join("workspace.lock"))?;
        let path = directory.join("workspace.sock");
        match lock.try_lock() {
            Ok(()) => {
                // Only the lock owner may remove a socket left by a crash.
                match fs::remove_file(&path) {
                    Ok(()) => {},
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error),
                }
                let socket = UnixDatagram::bind(&path)?;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                socket.set_nonblocking(true)?;
                Ok(Some(Self { _lock: lock, socket, path }))
            }
            Err(fs::TryLockError::WouldBlock) => {
                let socket = UnixDatagram::unbound()?;
                let payload = serde_json::to_vec(files)?;
                if payload.len() > 60_000 {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "Too many files in one open request"));
                }
                let mut last_error = None;
                for _ in 0..20 {
                    match socket.send_to(&payload, &path) {
                        Ok(_) => return Ok(None),
                        Err(error) => last_error = Some(error),
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(last_error.unwrap_or_else(|| io::Error::other("Existing workspace did not accept the request")))
            }
            Err(fs::TryLockError::Error(error)) => Err(error),
        }
    }

    pub fn requests(&self) -> Vec<Vec<String>> {
        let mut requests = Vec::new();
        let mut buffer = [0u8; 65_536];
        // Bound GUI work even if another process floods the socket.
        for _ in 0..16 {
            match self.socket.recv(&mut buffer) {
                Ok(size) => {
                    if let Ok(files) = serde_json::from_slice::<Vec<String>>(&buffer[..size]) {
                        requests.push(files);
                    }
                }
                Err(_) => break,
            }
        }
        requests
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // The lock remains held until after unlinking our own socket.
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_launch_forwards_without_creating_another_workspace() {
        let directory = std::env::temp_dir().join(format!("sculpt-instance-{}", std::process::id()));
        let first = Instance::acquire(&directory, &[]).unwrap().unwrap();
        let files = vec!["/tmp/My Session.scraft".to_string()];
        assert!(Instance::acquire(&directory, &files).unwrap().is_none());
        assert_eq!(first.requests(), vec![files]);
        drop(first);
        assert!(Instance::acquire(&directory, &[]).unwrap().is_some());
        let _ = fs::remove_dir_all(directory);
    }
}
