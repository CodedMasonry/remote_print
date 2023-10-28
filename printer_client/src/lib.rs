use std::{fs, io, net::ToSocketAddrs, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Result};
use app::Settings;
use quinn;
use rustls::Certificate;
use tokio::{fs::File, io::AsyncReadExt};
use tracing::{debug, error, info};
use url::Url;
use lazy_static::lazy_static;

pub mod app;

lazy_static! {
    static ref DEFAULT_ROOTS: Vec<Certificate> = {
        let mut temp = Vec::new();
        let dir_entries = fs::read_dir("../../certs").expect("Failed to read directory");

        for entry in dir_entries {
            if let Ok(entry) = entry {
                let file_path = entry.path();
                if let Ok(file_bytes) = fs::read(&file_path) {
                    let root_cert = Certificate(file_bytes);
                    temp.push(root_cert);
                }
            }
        }

        temp
    };
}

const ALPN_QUIC_HTTP: &[&[u8]] = &[b"hq-29"];

#[tokio::main]
pub async fn send_file(
    url: Url,
    host: Option<String>,
    ca: Option<PathBuf>,
    file: PathBuf,
) -> Result<()> {

    let remote = (url.host_str().unwrap(), url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("Couldn't resolve to an address"))?;

    // Parse for TLS Certs
    let mut roots = rustls::RootCertStore::empty();
    if let Some(ca_path) = ca {
        roots.add(&rustls::Certificate(fs::read(ca_path)?))?;
    } else {
        let dirs =
            directories_next::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();
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

        for cert in DEFAULT_ROOTS.clone().into_iter() {
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
    let file = file.clone();
    let headers = Vec::from([
        format!("POST {:?}", file.file_name().unwrap()),
        format!("Content-Length: {}", file.metadata().unwrap().len()),
        format!("Extension: {:?}", file.extension().unwrap()),
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
    eprintln!("Successfully sent file");
    println!("{}", String::from_utf8(resp).unwrap());

    conn.close(0u32.into(), b"done");

    endpoint.wait_idle().await;

    Ok(())
}

pub fn get_settings() -> Result<Settings> {
    let dirs = directories_next::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();

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
    let dirs = directories_next::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();
    let json = serde_json::to_string(&settings)?;

    // Write the json
    fs::write(dirs.data_local_dir().join("settings.json"), json)?;
    
    Ok(())
}