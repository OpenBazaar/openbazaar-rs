//! One Arti client whose state stays in a directory the node chooses.
//!
//! The default Arti folders live under the user's home directory. Two node
//! processes would then share one lock and one onion identity. Callers pass
//! the directory (`{data}/tor`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::Error;

const OPEN_ATTEMPTS: u32 = 4;
const OPEN_RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Another client in this process may be creating the same directory cache
/// (the node's remote-access client shares `{data}/tor`). Arti then reports
/// it locked or unreadable until that client is done, so this tries again.
pub fn open_client(
    state_dir: &Path,
) -> Result<Arc<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>, Error> {
    let (state, cache) = private_dirs(state_dir)?;
    let config = arti_client::config::TorClientConfigBuilder::from_directories(&state, &cache)
        .build()
        .map_err(|err| Error::Arti(err.to_string()))?;
    let open = || {
        arti_client::TorClient::builder()
            .config(config.clone())
            .bootstrap_behavior(arti_client::BootstrapBehavior::OnDemand)
            .create_unbootstrapped()
    };
    let mut result = open();
    for _ in 1..OPEN_ATTEMPTS {
        if result.is_ok() {
            break;
        }
        std::thread::sleep(OPEN_RETRY_WAIT);
        result = open();
    }
    result.map_err(|err| Error::Arti(err.to_string()))
}

/// `root/state` and `root/cache`, user-only from the moment they exist.
///
/// Arti refuses a state or cache directory other users can read. Two clients
/// in one process share `{data}/tor`; if one made these with the umask and
/// then narrowed them, the other could look in between and refuse to start.
pub fn private_dirs(root: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let state = root.join("state");
    let cache = root.join("cache");
    for dir in [&state, &cache] {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            builder.mode(0o700);
            builder.create(dir)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(not(unix))]
        builder.create(dir)?;
    }
    Ok((state, cache))
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn tor_dirs_are_user_only_when_made_and_when_found_open() {
        let root = std::env::temp_dir().join(format!("tormail-dirs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mode = |dir: &Path| fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        let (state, cache) = private_dirs(&root.join("tor")).unwrap();
        assert_eq!((mode(&state), mode(&cache)), (0o700, 0o700));
        assert_eq!(
            mode(&root.join("tor")) & 0o022,
            0,
            "no parent the client made is writable by others"
        );
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
        private_dirs(&root.join("tor")).unwrap();
        assert_eq!(mode(&state), 0o700, "an older, looser dir is narrowed");
        fs::remove_dir_all(root).unwrap();
    }
}
