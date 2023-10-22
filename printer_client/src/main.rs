use std::{
    fs,
    io,
    net::ToSocketAddrs,
    path::PathBuf,
    sync::Arc,
};

use anyhow::{anyhow, Result};
use clap::Parser;
use tokio::{fs::File, io::AsyncReadExt};
use tracing::{error, info, debug};
use tracing_subscriber;
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

    #[clap(short, long = "file")]
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

    // Parse for TLS Certs
    let mut roots = rustls::RootCertStore::empty();
    if let Some(ca_path) = args.ca {
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
    roots.add_trust_anchors(
        webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .map(|ta| {
                rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                    ta.subject,
                    ta.spki,
                    ta.name_constraints,
                )
            })
    );

    // TLS
    let mut client_crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();
    if args.keylog {
        client_crypto.key_log = Arc::new(rustls::KeyLogFile::new());
    }

    // Establish config and endpoint
    let client_config = quinn::ClientConfig::new(Arc::new(client_crypto));
    let mut endpoint = quinn::Endpoint::client("[::]:44536".parse().unwrap())?;
    endpoint.set_default_client_config(client_config);

    // Parse headers and file
    let headers = Vec::from([
        format!("POST {:?}", args.file.file_name().unwrap()),
        format!("Content-Length: {}", args.file.metadata().unwrap().len()),
        format!("Extension: {:?}", args.file.extension().unwrap()),
        format!("\r\n"),
    ])
    .join("\r\n");

    let mut buf = Vec::new();
    File::open(args.file).await?.read_to_end(&mut buf).await?;

    // convert request to binary
    let mut request = headers.into_bytes();
    request.extend(buf);

    // Resolve host name
    let host = args
        .host
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