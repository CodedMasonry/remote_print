use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use quinn::RecvStream;
use rand::distributions::{Alphanumeric, DistString};
use rustls::{Certificate, PrivateKey};
use tokio::{
    fs::{self, File},
    io::{self, AsyncBufReadExt, BufReader},
    process::Command,
};

use tracing::{debug, error, info, info_span, Instrument};
use tracing_subscriber;

const ALPN_QUIC_HTTP: &[&[u8]] = &[b"hq-29"];

#[derive(Parser, Debug)]
struct Args {
    /// TLS private key in PEM format
    #[clap(short = 'k', long = "key", requires = "cert")]
    key: Option<PathBuf>,

    /// TLS certificate in PEM format
    #[clap(short = 'c', long = "cert", requires = "key")]
    cert: Option<PathBuf>,

    /// Address to listen on
    #[clap(short, long = "listen", default_value = "0.0.0.0:4433")]
    listen: SocketAddr,

    /// Printer to use; If not set, uses default
    #[arg(short, long)]
    printer: Option<String>,
}

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
    let (cert, key) = parse_cert(args.key, args.cert).await?;
    debug!("Certificate and Key Parsed Successfully");

    let printer = Arc::new(args.printer.clone());

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(cert, key)?;
    server_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();

    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(server_crypto));
    let transfer_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transfer_config.max_concurrent_uni_streams(5_u8.into());
    server_config.use_retry(true);

    let endpoint = quinn::Endpoint::server(server_config, args.listen)?;
    eprintln!("Listening on {}", endpoint.local_addr()?);

    while let Some(conn) = endpoint.accept().await {
        info!("connection incoming");
        let handle = handle_connection(printer.clone(), conn);
        tokio::spawn(async move {
            if let Err(e) = handle.await {
                error!("connection failed: {reason}", reason = e.to_string())
            }
        });
    }

    Ok(())
}

async fn handle_connection(printer: Arc<Option<String>>, conn: quinn::Connecting) -> Result<()> {
    let connection = conn.await?;
    let span = info_span!(
        "connection",
        remote = %connection.remote_address(),
        protocol = %connection
            .handshake_data()
            .unwrap()
            .downcast::<quinn::crypto::rustls::HandshakeData>().unwrap()
            .protocol
            .map_or_else(|| "<none>".into(), |x| String::from_utf8_lossy(&x).into_owned())
    );

    async {
        info!("established");

        // Each stream initiated by the client constitutes a new request.
        loop {
            let stream = connection.accept_bi().await;
            let stream = match stream {
                Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                    info!("connection closed");
                    return Ok(());
                }
                Err(e) => {
                    return Err(e);
                }
                Ok(s) => s,
            };
            let fut = handle_request(printer.clone(), stream);
            tokio::spawn(
                async move {
                    if let Err(e) = fut.await {
                        error!("failed: {reason}", reason = e.to_string());
                    }
                }
                .instrument(info_span!("request")),
            );
        }
    }
    .instrument(span)
    .await?;

    Ok(())
}

async fn handle_request(
    printer: Arc<Option<String>>,
    (mut send, recv): (quinn::SendStream, quinn::RecvStream),
) -> Result<()> {
    let resp = process_request(printer.as_ref(), recv)
        .await
        .unwrap_or_else(|e| {
            error!("Failed: {}", e);
            format!("Failed to process request: {e}\n").into_bytes()
        });

    // Write result of handling and send finish
    send.write_all(&resp)
        .await
        .map_err(|e| anyhow!("failed to send response: {}", e))?;
    send.finish()
        .await
        .map_err(|e| anyhow!("failed to shutdown stream: {}", e))?;

    Ok(())
}

async fn process_request(printer: &Option<String>, recv: RecvStream) -> Result<Vec<u8>> {
    let mut reader = BufReader::new(recv);
    let mut name = String::new();
    loop {
        let r = reader.read_line(&mut name).await.unwrap();
        if r < 3 {
            break;
        }
    }

    let mut extension = String::new();
    let mut request_context = String::new();
    let linesplit = name.split("\n");
    // Parse some headers
    for l in linesplit {
        if l.starts_with("Extension") {
            let sizeplit = l.split(":");
            for s in sizeplit {
                if !(s.starts_with("Extension")) {
                    extension = s.trim().parse::<String>().unwrap(); //Get Content-Length
                }
            }
        } else if l.starts_with("POST") {
            request_context = String::from("print")
        } else if l.starts_with("GET") && l.contains("auth") {
            request_context = String::from("auth")
        }
    }

    if request_context == String::from("print") {
        print_file(printer, reader, extension).await
    } else if request_context == String::from("auth") {
        auth_user().await
    } else {
        bail!("Invalid Request")
    }
}

async fn print_file(
    printer: &Option<String>,
    mut reader: BufReader<RecvStream>,
    extension: String,
) -> Result<Vec<u8>> {
    debug!("Entension: {}", extension);

    // Create temp file
    let temp_name = Alphanumeric.sample_string(&mut rand::thread_rng(), 16);
    let dir = format!("/tmp/{}.{}", temp_name, extension);
    let mut file = File::create(dir.clone()).await?;
    debug!(file = dir);

    // Copy body to file
    tokio::io::copy(&mut reader, &mut file).await?;
    debug!("Successfully copied to file");

    // Print
    debug!(printer = printer);
    let result = if printer.is_some() {
        Command::new("lpr")
            .arg(dir)
            .arg("-P")
            .arg(printer.as_ref().unwrap())
            .output()
            .await?
    } else {
        // Use Default
        Command::new("lpr").arg(dir).output().await?
    };

    // If success, return done, else, return output
    if result.status.success() {
        Ok(b"done".to_vec())
    } else {
        let err = String::from_utf8(result.stderr).unwrap();
        // If no printer was found, notify User
        if err.contains("not exist") {
            let printers =
                String::from_utf8(Command::new("lpstat").arg("-p").output().await?.stdout)?;
            let printers: Vec<&str> = printers
                .lines()
                .map(|e| e.split(" ").nth(1).unwrap())
                .collect();
            error!(
                "Please specify a printer or set a default printer, Here are available printers: \n{:#?}",
                printers
            );
        }

        bail!("{:?}", err)
    }
}

async fn auth_user() -> Result<Vec<u8>> {
    bail!("Not implemented")
}

// Parse cert and keys
async fn parse_cert(
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
