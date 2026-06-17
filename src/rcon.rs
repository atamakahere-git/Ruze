use mc_rcon::RconClient;

/// Connect to a Minecraft RCON server.
///
/// # Errors
///
/// Returns `std::io::Error` if the TCP connection fails.
///
/// Logs a warning if authentication fails; the client is still returned so the
/// caller can decide how to handle it.
pub fn connect(address: &str, password: &str) -> std::io::Result<RconClient> {
    let client = RconClient::connect(address.to_string()).map_err(|e| {
        tracing::error!("Unable to connect to Minecraft RCON server: {e}");
        std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "RCON connection failed")
    })?;

    if let Err(e) = client.log_in(password) {
        tracing::error!("Failed to authenticate with RCON server: {e}");
    }

    Ok(client)
}
