//! One Arti client whose state stays in a directory the node chooses.
//!
//! The default Arti folders live under the user's home directory. Two node
//! processes would then share one lock and one onion identity. Callers pass
//! the directory (`{data}/tor`).

use std::fs;
use std::path::Path;
use std::sync::Arc;

use crate::Error;

pub(crate) fn open_client(
    state_dir: &Path,
) -> Result<Arc<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>, Error> {
    let state = state_dir.join("state");
    let cache = state_dir.join("cache");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&cache)?;
    tighten(&state)?;
    tighten(&cache)?;
    let config = arti_client::config::TorClientConfigBuilder::from_directories(&state, &cache)
        .build()
        .map_err(|err| Error::Arti(err.to_string()))?;
    arti_client::TorClient::builder()
        .config(config)
        .bootstrap_behavior(arti_client::BootstrapBehavior::OnDemand)
        .create_unbootstrapped()
        .map_err(|err| Error::Arti(err.to_string()))
}

fn tighten(dir: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let _ = dir;
    Ok(())
}
