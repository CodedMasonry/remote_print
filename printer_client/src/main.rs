#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::{fs, io, net::ToSocketAddrs, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use printer_client::app::Interface;
use quinn;
use tokio::{fs::File, io::AsyncReadExt};
use tracing::{debug, error, info};
use tracing_subscriber;
use url::Url;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Upload to remote printer
    Upload {
        url: Url,

        /// Override hostname used for certificate verification
        #[arg(long = "host")]
        host: Option<String>,

        /// Custom certificate authority to trust, in DER format
        #[arg(long = "ca")]
        ca: Option<PathBuf>,

        /// The File to send
        #[arg(short, long = "file")]
        file: PathBuf,
    },

    /// Open the GUI
    Gui {},
}

const ALPN_QUIC_HTTP: &[&[u8]] = &[b"hq-29"];

// Init tracing
fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    match args.command {
        Commands::Upload {
            url,
            host,
            ca,
            file,
        } => {
            send_file(url, host, ca, file)?;
        }

        Commands::Gui {} => run_gui()?,
    };

    Ok(())
}

fn run_gui() -> Result<()> {
    let options = eframe::NativeOptions {
        initial_window_size: Some([400.0, 500.0].into()),
        min_window_size: Some([300.0, 400.0].into()),
        drag_and_drop_support: true,
        ..Default::default()
    };
    eframe::run_native(
        "Remote Print",
        options,
        Box::new(|_cc| Box::<Interface>::default()),
    )
    .unwrap_or_else(|e| error!("Failed to run GUI: {}", e));

    Ok(())
}

#[tokio::main]
async fn send_file(
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
    }

    // TLS
    let mut client_crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();

    // Establish config
    let client_config = quinn::ClientConfig::new(Arc::new(client_crypto));
    let mut endpoint = quinn::Endpoint::client("[::]:44536".parse().unwrap())?;
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
