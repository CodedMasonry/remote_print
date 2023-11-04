#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use remote_printer_client::app::Interface;

use tracing::error;
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
            remote_printer_client::send_file(url, host, ca, file, None)?;
        }

        Commands::Gui {} => run_gui()?,
    };

    Ok(())
}

fn run_gui() -> Result<()> {
    let options = eframe::NativeOptions {
        initial_window_size: Some([450.0, 500.0].into()),
        min_window_size: Some([400.0, 500.0].into()),
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
