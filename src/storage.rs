use std::path::{Path, PathBuf};

use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};

use crate::consts;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("serialization failed: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("deserialization failed: {0}")]
    Deserialize(#[from] toml::de::Error),
    #[error("I/O error at `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("storage worker panicked: {0}")]
    BlockingPanic(String),
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct BridgeState {
    bridged_channels: Vec<u64>,
}

pub async fn load_channels() -> Result<Vec<serenity::ChannelId>, StorageError> {
    let path = consts::bridge_state_path();
    let path_display = path.display().to_string();

    let result = tokio::task::spawn_blocking(move || read_bridge_file(&path)).await;

    match result {
        Ok(Ok(state)) => {
            let channels: Vec<_> = state
                .bridged_channels
                .into_iter()
                .map(serenity::ChannelId::new)
                .collect();
            tracing::info!(
                count = channels.len(),
                path = %path_display,
                "loaded bridge state"
            );
            Ok(channels)
        }
        Ok(Err(StorageError::Io { source, .. }))
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            tracing::debug!(
                path = %path_display,
                "bridge state file not found, starting fresh"
            );
            Ok(Vec::new())
        }
        Ok(Err(e)) => {
            tracing::warn!(%e, path = %path_display, "failed to load bridge state, starting fresh");
            Ok(Vec::new())
        }
        Err(join_err) => Err(StorageError::BlockingPanic(join_err.to_string())),
    }
}

pub async fn save_channels(channels: Vec<u64>) -> Result<(), StorageError> {
    let path = consts::bridge_state_path();
    let count = channels.len();

    tracing::debug!(
        count,
        path = %path.display(),
        "persisting bridge state"
    );

    let result = tokio::task::spawn_blocking(move || write_bridge_file(&path, &channels)).await;

    match result {
        Ok(Ok(())) => {
            tracing::debug!(count, "bridge state saved");
            Ok(())
        }
        Ok(Err(e)) => {
            tracing::error!(%e, "failed to save bridge state");
            Err(e)
        }
        Err(join_err) => Err(StorageError::BlockingPanic(join_err.to_string())),
    }
}

fn read_bridge_file(path: &Path) -> Result<BridgeState, StorageError> {
    let content = std::fs::read_to_string(path).map_err(|e| StorageError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(toml::from_str(&content)?)
}

fn write_bridge_file(path: &Path, channels: &[u64]) -> Result<(), StorageError> {
    let state = BridgeState {
        bridged_channels: channels.to_vec(),
    };

    let serialized = toml::to_string(&state)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| StorageError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &serialized).map_err(|e| StorageError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    std::fs::rename(&tmp, path).map_err(|e| StorageError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ruze_test_{name}_{}.toml", std::process::id()))
    }

    #[test]
    fn roundtrip_save_and_load() {
        let path = temp_path("roundtrip");
        let channels = vec![123, 456, 789];

        write_bridge_file(&path, &channels).unwrap();
        let state = read_bridge_file(&path).unwrap();

        let _ = std::fs::remove_file(&path);
        assert_eq!(state.bridged_channels, channels);
    }

    #[test]
    fn load_non_existent_returns_not_found() {
        let path = PathBuf::from("/tmp/nonexistent_ruze_test_file_xyz123.toml");
        let err = read_bridge_file(&path).unwrap_err();
        match err {
            StorageError::Io { source, .. } => {
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            _ => panic!("expected Io error, got {err:?}"),
        }
    }

    #[test]
    fn load_corrupt_file_returns_deserialize_error() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "not valid toml {{{").unwrap();
        let err = read_bridge_file(&path).unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert!(matches!(err, StorageError::Deserialize(_)));
    }

    #[test]
    fn write_creates_parent_directories() {
        let dir = std::env::temp_dir().join(format!("ruze_test_dir_{}", std::process::id()));
        let path = dir.join("sub").join("bridge.toml");

        write_bridge_file(&path, &[42]).unwrap();

        let exists = path.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(exists);
    }

    #[test]
    fn write_does_not_leave_temp_file() {
        let path = temp_path("no_tmp");
        let tmp = path.with_extension("tmp");

        write_bridge_file(&path, &[1, 2]).unwrap();

        let tmp_exists = tmp.exists();
        let _ = std::fs::remove_file(&path);
        assert!(!tmp_exists);
    }
}
