use mc_rcon::RconClient;

/// Errors that can occur when connecting to or authenticating with a Minecraft RCON server.
#[derive(Debug, thiserror::Error)]
pub enum RconError {
    #[error("rcon connection failed: {0}")]
    Connect(#[from] std::io::Error),
    #[error("rcon authentication failed: {0}")]
    Auth(String),
}

/// Connect to a Minecraft RCON server.
///
/// # Errors
///
/// Returns `RconError::Connect` if the TCP connection fails, or `RconError::Auth`
/// if the login credentials are rejected.
pub fn connect(address: &str, password: &str) -> Result<RconClient, RconError> {
    let client = RconClient::connect(address.to_string())
        .inspect_err(|e| tracing::error!("unable to connect to RCON at {address}: {e}"))?;

    client
        .log_in(password)
        .inspect_err(|e| tracing::error!("rcron authentication failed: {e}"))
        .map_err(|e| RconError::Auth(e.to_string()))?;

    tracing::info!("RCON connected to {address}");
    Ok(client)
}
