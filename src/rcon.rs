use std::sync::Mutex;

use mc_rcon::RconClient;

/// Errors that can occur when connecting to or using a Minecraft RCON server.
#[derive(Debug, thiserror::Error)]
pub enum RconError {
    #[error("rcon connection failed: {0}")]
    Connect(#[from] std::io::Error),
    #[error("rcon error: {0}")]
    Rcon(String),
}

/// An RCON client wrapper that automatically reconnects on send failure.
///
/// Uses an internal `std::sync::Mutex` so all callers can share a single
/// `Arc<ReconnectingRcon>` without an external lock.
pub struct ReconnectingRcon {
    address: String,
    password: String,
    client: Mutex<Option<RconClient>>,
}

impl ReconnectingRcon {
    /// Connect to a Minecraft RCON server.
    pub fn connect(address: String, password: String) -> Result<Self, RconError> {
        let client = Self::create_client(&address, &password)?;
        tracing::info!("RCON connected to {address}");
        Ok(Self {
            address,
            password,
            client: Mutex::new(Some(client)),
        })
    }

    /// Send an RCON command, reconnecting automatically if the existing
    /// connection has failed.
    pub fn send_command(&self, command: &str) -> Result<String, RconError> {
        {
            let guard = self.client.lock().unwrap();
            if let Some(ref client) = *guard {
                match client.send_command(command) {
                    Ok(result) => return Ok(result),
                    Err(e) => {
                        tracing::warn!("rcon send failed: {e}, will attempt reconnect");
                    }
                }
            }
        }

        tracing::info!("reconnecting RCON to {}...", self.address);
        let mut guard = self.client.lock().unwrap();
        match Self::create_client(&self.address, &self.password) {
            Ok(new_client) => {
                let result = new_client.send_command(command);
                *guard = Some(new_client);
                result.map_err(|e| RconError::Rcon(format!("{e}")))
            }
            Err(e) => {
                *guard = None;
                Err(e)
            }
        }
    }

    fn create_client(address: &str, password: &str) -> Result<RconClient, RconError> {
        let client = RconClient::connect(address.to_string())?;
        client
            .log_in(password)
            .map_err(|e| RconError::Rcon(format!("{e}")))?;
        Ok(client)
    }
}
