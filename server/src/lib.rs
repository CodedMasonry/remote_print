use anyhow::{bail, Context, Result};
use quinn::RecvStream;
use rustls::{self, Certificate, PrivateKey};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use uuid::Uuid;

use chrono::prelude::*;
use chrono::Duration;
use lazy_static::lazy_static;
use orion::{self, pwhash};
use serde;
use tokio::{
    fs,
    io::{self, AsyncReadExt, BufReader},
    sync::Mutex,
};
use tracing::{error, info};

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Settings {
    pub hash: pwhash::PasswordHash,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct Session {
    pub expiratrion: DateTime<Utc>,
}

lazy_static! {
    // Sessions are not intended to be persistent
    // Sessions should only last a few hours at maximum
    pub static ref SESSION_STORAGE: Arc<Mutex<HashMap<Uuid, Session>>> =
        Arc::new(Mutex::from(HashMap::new()));
}

impl Session {
    pub fn new() -> Self {
        Session {
            expiratrion: Utc::now() + Duration::hours(4),
        }
    }
}

impl Settings {
    pub async fn get_settings() -> Result<Settings> {
        let dirs = directories::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();

        let settings = match fs::read(dirs.data_local_dir().join("server_settings.json")).await {
            Ok(file) => {
                let settings: Settings = serde_json::from_slice(&file)?;
                settings
            }
            Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
                info!("local Settings not found, returning default");
                let settings = Settings::build()?;

                Settings::save_settings(&settings).await?;
                settings
            }
            Err(e) => {
                error!(
                    "failed to open settings: {}\nUsing default settings (Saving disabled)",
                    e
                );
                Settings::build()?
            }
        };

        Ok(settings)
    }
    pub async fn save_settings(settings: &Settings) -> Result<()> {
        let dirs = directories::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();
        let json = serde_json::to_string(&settings)?;

        // Make sure directories exist
        fs::create_dir_all(dirs.data_local_dir()).await?;
        // Write the json
        fs::write(dirs.data_local_dir().join("server_settings.json"), json).await?;

        Ok(())
    }

    pub fn build() -> Result<Self> {
        println!("A password is needed for clients to connect");
        let pass = inquire::Password::new("Please enter a password:")
            .with_display_toggle_enabled()
            .with_display_mode(inquire::PasswordDisplayMode::Hidden)
            .with_custom_confirmation_message("Confirm Password:")
            .with_custom_confirmation_error_message("Passwords do not match")
            .prompt()?;

        let password = pwhash::Password::from_slice(pass.as_bytes())?;
        drop(pass); // Want the raw password in memory for as little time as possible

        let hash = pwhash::hash_password(&password, 3, 1 << 16)?;

        Ok(Self { hash })
    }
}

// Parse cert and keys
pub async fn parse_tls_cert(
    key: Option<PathBuf>,
    cert: Option<PathBuf>,
) -> Result<(Vec<Certificate>, PrivateKey)> {
    if let (Some(key_path), Some(cert_path)) = (key, cert) {
        let key = fs::read(key_path.clone())
            .await
            .context("failed to read private key")?;
        let key = if key_path.extension().map_or(false, |x| x == "der") {
            rustls::PrivateKey(key)
        } else {
            let pkcs8 = rustls_pemfile::pkcs8_private_keys(&mut &*key)
                .context("malformed PKCS #8 private key")?;
            match pkcs8.into_iter().next() {
                Some(x) => rustls::PrivateKey(x),
                None => {
                    let rsa = rustls_pemfile::rsa_private_keys(&mut &*key)
                        .context("malformed PKCS #1 private key")?;
                    match rsa.into_iter().next() {
                        Some(x) => rustls::PrivateKey(x),
                        None => {
                            anyhow::bail!("no private keys found");
                        }
                    }
                }
            }
        };
        let cert_chain = fs::read(cert_path.clone())
            .await
            .context("failed to read certificate chain")?;
        let cert_chain = if cert_path.extension().map_or(false, |x| x == "der") {
            vec![rustls::Certificate(cert_chain)]
        } else {
            rustls_pemfile::certs(&mut &*cert_chain)
                .context("invalid PEM-encoded certificate")?
                .into_iter()
                .map(rustls::Certificate)
                .collect()
        };

        Ok((cert_chain, key))
    } else {
        let dirs = directories::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();
        let path = dirs.data_local_dir();
        let cert_path = path.join("cert.der");
        let key_path = path.join("key.der");
        let (cert, key) = match fs::read(&cert_path)
            .await
            .and_then(|x| Ok((x, std::fs::read(&key_path)?)))
        {
            Ok(x) => x,
            Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
                info!("generating self-signed certificate");
                let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
                let key = cert.serialize_private_key_der();
                let cert = cert.serialize_der().unwrap();
                fs::create_dir_all(path)
                    .await
                    .context("failed to create certificate directory")?;
                fs::write(&cert_path, &cert)
                    .await
                    .context("failed to write certificate")?;
                fs::write(&key_path, &key)
                    .await
                    .context("failed to write private key")?;
                (cert, key)
            }
            Err(e) => {
                bail!("failed to read certificate: {}", e);
            }
        };

        let key = rustls::PrivateKey(key);
        let cert = rustls::Certificate(cert);
        Ok((vec![cert], key))
    }
}

/// Attempts to create a session.
/// Fails if password doesn't match
pub async fn init_session(
    hash: &pwhash::PasswordHash,
    mut reader: BufReader<RecvStream>,
) -> Result<Vec<u8>> {
    let mut pass = Vec::new();
    reader.read_to_end(&mut pass).await?;

    let password = pwhash::Password::from_slice(&pass)?;

    // Implement fail timeout later
    // Register session if success, return result of verification
    match pwhash::hash_password_verify(hash, &password) {
        Ok(_) => {
            // Initialize new connection
            // Generate UUID on server because you should never trust the client
            let mut lock = SESSION_STORAGE.lock().await;
            let session_id = Uuid::new_v4();
            let session = Session::new();

            lock.insert(session_id, session.clone());
            drop(lock); // Explicit release

            // Success & Id & Expiratrion
            // Designed for client handling
            let result = format!("success&{}&{}", session_id, session.expiratrion)
                .as_bytes()
                .to_vec();
            return Ok(result);
        }
        Err(_) => {
            bail!("Invalid Password");
        }
    };
}
