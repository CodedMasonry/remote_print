use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{anyhow, bail, Result};
use chrono::Utc;
use clap::Parser;
use printer_server::Settings;
use quinn::RecvStream;
use rand::distributions::{Alphanumeric, DistString};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

use printer_server;
use tracing::{debug, error, info, info_span, Instrument};
use tracing_subscriber;
use uuid::Uuid;

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

    /// Reset password
    #[arg(long)]
    reset_password: bool,
}

// Init tracing
fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    if args.reset_password {
        let dirs =
            directories_next::ProjectDirs::from("com", "Coded Masonry", "Remote Print").unwrap();
        match std::fs::remove_file(dirs.data_local_dir().join("server_settings.json")) {
            Ok(_) => println!("Password reset"),
            Err(_) => println!("No password was saved"),
        };
    }

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
    let (cert, key) = printer_server::parse_tls_cert(args.key, args.cert).await?;
    debug!("Certificate and Key Parsed Successfully");

    let settings = Arc::new(printer_server::Settings::get_settings().await?);
    debug!("Settings parsed successfully");

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
        let handle = handle_connection(printer.clone(), settings.clone(), conn);
        tokio::spawn(async move {
            if let Err(e) = handle.await {
                error!("connection failed: {reason}", reason = e.to_string())
            }
        });
    }

    Ok(())
}

async fn handle_connection(
    printer: Arc<Option<String>>,
    settings: Arc<Settings>,
    conn: quinn::Connecting,
) -> Result<()> {
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
            let fut = handle_request(printer.clone(), settings.clone(), stream);
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
    settings: Arc<Settings>,
    (mut send, recv): (quinn::SendStream, quinn::RecvStream),
) -> Result<()> {
    let resp = process_request(printer.as_ref(), settings, recv)
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

async fn process_request(
    printer: &Option<String>,
    settings: Arc<Settings>,
    recv: RecvStream,
) -> Result<Vec<u8>> {
    let mut reader = BufReader::new(recv);
    let mut name = String::new();
    loop {
        let r = reader.read_line(&mut name).await.unwrap();
        if r < 3 {
            break;
        }
    }

    let mut extension = String::new();
    let mut session_id = String::new();
    let mut request_context = String::new();
    let linesplit = name.split("\n");
    // Parse some headers
    for l in linesplit {
        if l.starts_with("Extension") {
            // Extension Header
            let sizeplit = l.split(":");
            for s in sizeplit {
                if !(s.starts_with("Extension")) {
                    extension = s.trim().parse::<String>().unwrap();
                }
            }
        } else if l.starts_with("Session") {
            // Session Header
            let sizeplit = l.split(":");
            for s in sizeplit {
                if !(s.starts_with("Session")) {
                    session_id = s.trim().parse::<String>().unwrap();
                }
            }
        } else if l.starts_with("POST") {
            // if POST
            request_context = String::from("print")
        } else if l.starts_with("GET") && l.contains("auth") {
            // if AUTH
            request_context = String::from("auth")
        }
    }

    if request_context == String::from("print") {
        let lock = printer_server::SESSION_STORAGE.lock().await;
        let id = Uuid::parse_str(&session_id)?;

        // Checks if session exists
        if let Some(session) = lock.get(&id) {
            if session.expiratrion < Utc::now() {
                bail!("Expired Session")
            }
        } else {
            bail!("Authentication Required")
        }

        drop(lock); // Explicit release
        print_file(printer, reader, extension).await
    } else if request_context == String::from("auth") {
        printer_server::init_session(&settings.hash, reader).await
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
        let temp = Command::new("lpr")
            .arg(dir.clone())
            .arg("-P")
            .arg(printer.as_ref().unwrap())
            .output()
            .await;
        // If it failed, try using lp instead
        match temp {
            Ok(output) => output,
            Err(_) => {
                Command::new("lp")
                    .arg(dir)
                    .arg("-d")
                    .arg(printer.as_ref().unwrap())
                    .output()
                    .await?
            }
        }
    } else {
        // Use Default (only works if lpr exists)
        let temp = Command::new("lpr").arg(dir.clone()).output().await;
        match temp {
            Ok(o) => o,
            Err(_) => {
                Command::new("lp")
                    .arg(dir)
                    .output()
                    .await?
            }
        }
    };

    // If success, return done, else, return output
    if result.status.success() {
        Ok(b"done".to_vec())
    } else {
        let err = String::from_utf8(result.stderr)?;
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
