use std::{
    fs,
    io::{self, Write},
    net::ToSocketAddrs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use clap::Parser;
use tracing::{error, info};
use tracing_subscriber::fmt::format;
use url::Url;

#[derive(Parser, Debug)]
struct Args {
    /// Perform NSS-compatible TLS key logging to the file specified in `SSLKEYLOGFILE`.
    #[clap(long = "keylog")]
    keylog: bool,

    url: Url,

    /// Override hostname used for certificate verification
    #[clap(long = "host")]
    host: Option<String>,

    /// Custom certificate authority to trust, in DER format
    #[clap(long = "ca")]
    ca: Option<PathBuf>,

    /// Simulate NAT rebinding after connecting
    #[clap(long = "rebind")]
    rebind: bool,

    #[clap(long = "file")]
    file: PathBuf,
}

const ALPN_QUIC_HTTP: &[&[u8]] = &[b"hq-29"];

// Init tracing
fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let code = {
        if let Err(e) = run(args) {
            eprintln!("ERROR: {e}");
            1
        } else {
            0
        }
    };

    std::process::exit(code);
}

// main func
#[tokio::main]
async fn run(args: Args) -> Result<()> {
    let url = args.url;
    let remote = (url.host_str().unwrap(), url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("Couldn't resolve to an address"))?;

    let mut roots = rustls::RootCertStore::empty();
    if let Some(ca_path) = args.ca {
        roots.add(&rustls::Certificate(fs::read(ca_path)?))?;
    } else {
        let dirs = directories_next::ProjectDirs::from("org", "quinn", "quinn-examples").unwrap();
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

    let mut client_crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();
    if args.keylog {
        client_crypto.key_log = Arc::new(rustls::KeyLogFile::new());
    }

    let client_config = quinn::ClientConfig::new(Arc::new(client_crypto));
    let mut endpoint = quinn::Endpoint::client("[::]:0".parse().unwrap())?;
    endpoint.set_default_client_config(client_config);

    let headers = Vec::from([
        format!("POST {:?}", args.file.file_name()),
        format!("Content-Length: {}", args.file.metadata().unwrap().len()),
        format!("Extension: {:?}", args.file.extension()),
    ])
    .join("\r\n");

    let start = Instant::now();
    let rebind = args.rebind;
    let host = args
        .host
        .as_ref()
        .map_or_else(|| url.host_str(), |x| Some(x))
        .ok_or_else(|| anyhow!("no hostname specified"))?;

    eprintln!("Connecting to {host} at {remote}");
    let conn = endpoint
        .connect(remote, host)?
        .await
        .map_err(|e| anyhow!("Failed to connect: {}", e))?;
    eprintln!("Connected at {:?}", start.elapsed());

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow!("Failed to open stream: {}", e))?;

    // Nat rebinding (Idk what it does)
    if rebind {
        let socket = std::net::UdpSocket::bind("[::]:0").unwrap();
        let addr = socket.local_addr().unwrap();
        eprintln!("rebinding to {addr}");
        endpoint.rebind(socket).expect("rebind failed");
    }

    // Send Headers
    send.write_all(&headers.as_bytes())
        .await
        .map_err(|e| anyhow!("Failed to send request: {}", e))?;
    
    send.finish()
        .await
        .map_err(|e| anyhow!("failed to shut down stream: {}", e))?;

    // Read response
    let response_start = Instant::now();
    eprintln!("request sent at {:?}", response_start - start);
    let resp = recv
        .read_to_end(usize::max_value())
        .await
        .map_err(|e| anyhow!("failed to read response: {}", e))?;
    let duration = response_start.elapsed();
    eprintln!(
        "response received in {:?} - {} KiB/s",
        duration,
        resp.len() as f32 / (duration_secs(&duration) * 1024.0)
    );

    io::stdout().write_all(&resp).unwrap();
    io::stdout().flush().unwrap();
    conn.close(0u32.into(), b"done");

    endpoint.wait_idle().await;

    Ok(())
}

fn duration_secs(x: &Duration) -> f32 {
    x.as_secs() as f32 + x.subsec_nanos() as f32 * 1e-9
}