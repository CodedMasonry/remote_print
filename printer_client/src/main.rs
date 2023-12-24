#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use printer_client::app::Interface;

use tracing::error;
use tracing_subscriber::{self};
use url::Url;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
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
}

// Init tracing
fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    if args.command.is_none() {
        run_gui()?;
    } else {
        match args.command.unwrap() {
            Commands::Upload {
                url,
                host,
                ca,
                file,
            } => {
                printer_client::send_file(url, host, ca, file, None)?;
                true
            }
        };
    }
    Ok(())
}

fn run_gui() -> Result<()> {
    println!("Running version: {}", env!("CARGO_PKG_VERSION"));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([450.0, 500.0]) // wide enough for the drag-drop overlay text
            .with_drag_and_drop(true)
            .with_min_inner_size([400.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Printer Client",
        options,
        Box::new(|_cc| Box::<Interface>::default()),
    )
    .unwrap_or_else(|e| error!("Failed to run GUI: {}", e));

    Ok(())
}