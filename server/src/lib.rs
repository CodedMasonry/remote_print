use anyhow::{bail, Context, Result};
use rustls::{self, Certificate, PrivateKey};
use std::path::PathBuf;

use orion::{self, pwhash};
use serde;
use tokio::{fs, io};
use tracing::{error, info};

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Settings {
    hash: pwhash::PasswordHash,
}

impl Settings {
    pub async fn get_settings() -> Result<Settings> {
        let dirs =
            directories_next::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();

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
                error!("failed to open settings: {}\nUsing default settings (Saving disabled)", e);
                Settings::build()?
            }
        };

        Ok(settings)
    }

    pub async fn save_settings(settings: &Settings) -> Result<()> {
        let dirs =
            directories_next::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();
        let json = serde_json::to_string(&settings)?;

        // Write the json
        fs::write(dirs.data_local_dir().join("server_settings.json"), json).await?;

        Ok(())
    }

    pub fn build() -> Result<Self> {
        let mut pass = String::new();

        print!("Please enter a password [Only need to do this once]: ");
        std::io::stdin()
            .read_line(&mut pass)
            .expect("failed to read line");

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
        let dirs =
            directories_next::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();
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
