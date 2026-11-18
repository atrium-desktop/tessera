//! One Settings window per session.
//!
//! The first instance binds `$XDG_RUNTIME_DIR/tessera-settings.sock`. A later
//! invocation connects instead, hands over its deep-link target as one line,
//! and exits, so every entry point (the Control Center, the launcher, the CLI)
//! lands in the same window on the requested page.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use tessera_wl_client::WakeSender as LoopSender;

/// Longest deep-link line accepted from a peer.
const MAX_TARGET_LEN: u64 = 128;

pub fn socket_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("tessera-settings.sock")
}

/// Outcome of claiming the per-session instance.
pub enum Claim {
    /// This process is the instance; requests from later invocations arrive
    /// through the listener.
    Primary(Listener),
    /// Another instance answered and received the target.
    Forwarded,
}

pub struct Listener {
    listener: UnixListener,
    path: PathBuf,
}

/// Removes the instance socket when the window's event loop ends.
pub struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Become the instance, or forward `target` to the one that already is.
pub fn claim(path: &Path, target: Option<&str>) -> io::Result<Claim> {
    match UnixStream::connect(path) {
        Ok(mut stream) => {
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            writeln!(stream, "{}", target.unwrap_or_default())?;
            return Ok(Claim::Forwarded);
        }
        // A socket file nobody listens on is a crashed instance's leftover.
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            std::fs::remove_file(path)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(Claim::Primary(Listener {
        listener,
        path: path.to_owned(),
    }))
}

impl Listener {
    /// Accept forwarded targets on a background thread. An empty line means
    /// "just show the window". The returned guard owns the socket file.
    pub fn serve(self, targets: LoopSender<String>) -> io::Result<Guard> {
        let Listener { listener, path } = self;
        thread::Builder::new()
            .name("settings-instance".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut line = String::new();
                    let read =
                        BufReader::new(io::Read::take(stream, MAX_TARGET_LEN)).read_line(&mut line);
                    if read.is_ok() && targets.send(line.trim().to_owned()).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Guard(path))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tessera_wl_client::{WakeSignal, wake_channel as channel_fn};

    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tessera-settings-instance-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_second_invocation_forwards_its_page_to_the_first() {
        let dir = scratch_dir("forward");
        let path = socket_path(&dir);
        let Claim::Primary(listener) = claim(&path, None).unwrap() else {
            panic!("the first claim must become the instance");
        };
        let (tx, rx) = channel_fn(WakeSignal::new().unwrap());
        let guard = listener.serve(tx).unwrap();
        assert!(matches!(
            claim(&path, Some("power")).unwrap(),
            Claim::Forwarded
        ));
        let target = std::iter::repeat_with(|| rx.try_recv())
            .take(200)
            .find_map(|received| {
                std::thread::sleep(Duration::from_millis(5));
                received.ok()
            });
        assert_eq!(target.as_deref(), Some("power"));
        drop(guard);
        assert!(!path.exists(), "the guard removes the socket");
    }

    #[test]
    fn a_stale_socket_is_reclaimed() {
        let dir = scratch_dir("stale");
        let path = socket_path(&dir);
        // A bound-then-dropped listener leaves a socket nobody accepts on.
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        assert!(matches!(claim(&path, None).unwrap(), Claim::Primary(_)));
    }
}
