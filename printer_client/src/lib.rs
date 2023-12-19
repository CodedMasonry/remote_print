use std::{
    fs, io,
    net::{SocketAddr, ToSocketAddrs},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, bail, Result};
use app::Settings;
use chrono::prelude::*;
use include_dir::{include_dir, Dir};
use inquire;
use quinn::{self, Connection, Endpoint};
use rustls::Certificate;
use tokio::{fs::File, io::AsyncReadExt, time::timeout};
use tracing::{debug, error, info, info_span, Instrument};
use url::Url;
use uuid::Uuid;

pub mod app;

static DEFAULT_ROOTS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/certs");

const ALPN_QUIC_HTTP: &[&[u8]] = &[b"hq-29"];

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct Printer {
    pub pass: String,
    pub session: Option<Session>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct Session {
    pub id: Uuid,
    pub expiratrion: DateTime<Utc>,
}

impl Printer {
    pub fn new(pass: String) -> Self {
        Printer {
            pass,
            session: None,
        }
    }
}

#[tokio::main]
pub async fn send_file(
    url: Url,
    host: Option<String>,
    ca: Option<PathBuf>,
    file: PathBuf,
    printer: Option<&mut Printer>,
) -> Result<()> {
    let remote = (url.host_str().unwrap(), url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("Couldn't resolve to an address"))?;

    // Parse for TLS Certs
    let mut roots = rustls::RootCertStore::empty();
    if let Some(ca_path) = ca.clone() {
        roots.add(&rustls::Certificate(fs::read(ca_path)?))?;
    } else {
        let dirs = directories::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();
        match fs::read(dirs.data_local_dir().join("cert.der")) {
            Ok(cert) => {
                roots.add(&rustls::Certificate(cert))?;
            }
            Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
                info!("local server certificate not found");
            }
            Err(e) => {
                error!("failed to open local server certificate: {}", e);
            }
        }

        for cert in parse_certs().await {
            debug!("Cert Added: {:#?}", cert);
            roots.add(&cert)?;
        }
    }

    // TLS
    let mut client_crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();

    // Establish config
    let client_config = quinn::ClientConfig::new(Arc::new(client_crypto));
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())?;
    endpoint.set_default_client_config(client_config);

    // Parse session
    let session = if let Some(temp) = printer {
        if let Some(session) = &temp.session {
            // Session exists
            if session.expiratrion <= Utc::now() {
                // Session expired
                get_session(url.clone(), host.clone(), ca.clone(), temp.pass.clone())
                    .instrument(info_span!("Fetch Session"))
                    .await?
            } else {
                // Session Valid
                session.clone()
            }
        } else {
            // No session exists
            let session = get_session(url.clone(), host.clone(), ca.clone(), temp.pass.clone())
                .instrument(info_span!("Fetch Session"))
                .await?;

            temp.session = Some(session.clone()); // Update session
            session
        }
    } else {
        // No Printer passed, generate temp session
        let pass = request_for_pass().await;
        get_session(url.clone(), host.clone(), ca.clone(), pass)
            .instrument(info_span!("Fetch Session"))
            .await?
    };

    // Parse headers and file
    let file = file.clone();
    let headers = Vec::from([
        format!("POST {:?}", file.file_name().unwrap()),
        format!("Content-Length: {}", file.metadata().unwrap().len()),
        format!("Extension: {:?}", file.extension().unwrap()),
        format!("Session: {}", session.id),
        format!("\r\n"),
    ])
    .join("\r\n");

    let mut buf = Vec::new();
    File::open(file).await?.read_to_end(&mut buf).await?;

    // convert request to binary
    let mut request = headers.into_bytes();
    request.extend(buf);

    // Resolve host name
    let host = host
        .as_ref()
        .map_or_else(|| url.host_str(), |x| Some(x))
        .ok_or_else(|| anyhow!("no hostname specified"))?;

    // Establish connection
    eprintln!("Connecting to {host} at {remote}");
    let timelimit = Duration::from_secs(5);
    let conn = timeout(timelimit, establish_conn(endpoint.clone(), remote, host)).await??;

    // Parse Reader & Writer
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow!("Failed to open stream: {}", e))?;

    // Send off request
    send.write_all(&request)
        .await
        .map_err(|e| anyhow!("Failed to send request: {}", e))?;

    send.finish()
        .await
        .map_err(|e| anyhow!("failed to shut down stream: {}", e))?;

    // Read response
    let resp = recv
        .read_to_end(usize::max_value())
        .await
        .map_err(|e| anyhow!("failed to read response: {}", e))?;
    eprintln!("Successfully sent file");
    println!("{}", String::from_utf8(resp).unwrap());

    conn.close(0u32.into(), b"done");

    endpoint.wait_idle().await;

    Ok(())
}

async fn establish_conn(endpoint: Endpoint, remote: SocketAddr, host: &str) -> Result<Connection> {
    let conn = endpoint
        .connect(remote, host)?
        .await
        .map_err(|e| anyhow!("Failed to connect: {}", e))?;
    debug!("Connected to server");

    Ok(conn)
}

pub async fn get_session(
    url: Url,
    host: Option<String>,
    ca: Option<PathBuf>,
    pass: String,
) -> Result<Session> {
    let remote = (url.host_str().unwrap(), url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("Couldn't resolve to an address"))?;

    // Parse for TLS Certs
    let mut roots = rustls::RootCertStore::empty();
    if let Some(ca_path) = ca {
        roots.add(&rustls::Certificate(fs::read(ca_path)?))?;
    } else {
        let dirs = directories::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();
        match fs::read(dirs.data_local_dir().join("cert.der")) {
            Ok(cert) => {
                roots.add(&rustls::Certificate(cert))?;
            }
            Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
                info!("local server certificate not found");
            }
            Err(e) => {
                error!("failed to open local server certificate: {}", e);
            }
        }

        for cert in parse_certs().await {
            debug!("Root Cert Added from certs directory");
            roots.add(&cert)?;
        }
    }

    // TLS
    let mut client_crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();

    // Establish config
    let client_config = quinn::ClientConfig::new(Arc::new(client_crypto));
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())?;
    endpoint.set_default_client_config(client_config);

    // Parse headers and file
    let headers = Vec::from([format!("GET authenticate"), format!("\r\n")]).join("\r\n");

    let mut request = headers.into_bytes();
    request.extend(pass.as_bytes());

    // Resolve host name
    let host = host
        .as_ref()
        .map_or_else(|| url.host_str(), |x| Some(x))
        .ok_or_else(|| anyhow!("no hostname specified"))?;

    // Establish connection
    eprintln!("Connecting to {host} at {remote}");
    let conn = endpoint
        .connect(remote, host)?
        .await
        .map_err(|e| anyhow!("Failed to connect: {}", e))?;
    debug!("Connected to server");

    // Parse Reader & Writer
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow!("Failed to open stream: {}", e))?;

    // Send off request
    send.write_all(&request)
        .await
        .map_err(|e| anyhow!("Failed to send request: {}", e))?;

    send.finish()
        .await
        .map_err(|e| anyhow!("failed to shut down stream: {}", e))?;

    // Read response
    let resp = recv
        .read_to_end(usize::max_value())
        .await
        .map_err(|e| anyhow!("failed to read response: {}", e))?;
    eprintln!("Successfully verified session");

    conn.close(0u32.into(), b"done");

    endpoint.wait_idle().await;

    let resp = String::from_utf8(resp)?;
    debug!(response = resp);

    let resp: Vec<&str> = resp.split("&").collect();
    if resp[0] == "success" {
        let session = Session {
            id: Uuid::parse_str(resp[1])?,
            expiratrion: DateTime::from_str(resp[2])?,
        };

        return Ok(session);
    } else {
        bail!("Failed: {}", resp[0])
    }
}

pub fn get_settings() -> Result<Settings> {
    let dirs = directories::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();

    let settings = match fs::read(dirs.data_local_dir().join("settings.json")) {
        Ok(file) => {
            let settings: Settings = serde_json::from_slice(&file)?;
            settings
        }
        Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
            info!("local Settings not found, returning default");
            Settings::new()
        }
        Err(e) => {
            error!("failed to open settings: {}\nUsing default settings", e);
            Settings::new()
        }
    };

    Ok(settings)
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    let dirs = directories::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();
    let json = serde_json::to_string(&settings)?;
    debug!("Fetching dir: {:?}", dirs.data_local_dir());

    // Make sure directories exist
    fs::create_dir_all(dirs.data_local_dir())?;
    // Write the json
    fs::write(dirs.data_local_dir().join("settings.json"), json)?;

    Ok(())
}

pub async fn parse_certs() -> Vec<Certificate> {
    let mut temp = Vec::new();
    for file in DEFAULT_ROOTS.files() {
        let root_cert = Certificate(file.contents().to_vec());

        temp.push(root_cert);
    }

    temp
}

/// Ask the user for the password (CLI Only)
pub async fn request_for_pass() -> String {
    let pass = inquire::Password::new("Please enter a password:")
        .with_display_toggle_enabled()
        .with_display_mode(inquire::PasswordDisplayMode::Hidden)
        .prompt()
        .unwrap();

    pass
}

pub fn update_app() -> Result<(), Box<dyn ::std::error::Error>> {
    use self_update::cargo_crate_version;
    let mut status_builder = self_update::backends::github::Update::configure();

    let status = status_builder
        .repo_owner("CodedMasonry")
        .repo_name("remote_print")
        .bin_name("printer_client")
        .show_download_progress(true)
        .no_confirm(true)
        .current_version(cargo_crate_version!())
        .build()?
        .update()?;

    println!("Update status: `{}`!", status.version());
    Ok(())
}
