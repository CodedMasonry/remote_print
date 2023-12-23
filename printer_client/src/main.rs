#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use printer_client::app::Interface;

use tracing::error;
use tracing_subscriber;
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
    if let Err(e) = update() {
        eprintln!("Failed to update: \n{:#?}", e);
    }

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

fn update() -> Result<(), Box<dyn std::error::Error>> {
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner("CodedMasonry")
        .repo_name("remote_print")
        .build()?
        .fetch()?;
    println!("found releases:");

    // get the first available release
    let asset = releases
        .into_iter()
        .filter(|val| val.name.contains("printer_client"))
        .next()
        .unwrap();

    //.asset_for(&self_update::get_target(), Some("printer_client-"))

    let mut installer = None;

    let files = asset
        .assets
        .into_iter()
        .filter(|val| val.name.contains("ps1") || val.name.contains("sh"))
        .filter(|val| !val.name.contains("sha"));

    // If the OS matches, installer will be set to it (Compiler flags will dictate this)
    for file in files {
        #[cfg(target_os = "windows")]
        if file.name.contains("ps1") {
            installer = Some(file);
        }

        #[cfg(target_os = "linux")]
        if file.name.contains("sh") {
            installer = Some(file);
        }

        // plan to add this later
        #[cfg(target_os = "macos")]
        if file {
            installer = None;
        }
    }

    if let None = installer {
        return Err(anyhow!("No installer for supported OS").into())
    }

    println!("Installer: {:#?}", installer);

    Ok(())
}
