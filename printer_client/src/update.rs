use std::process::{self, Command, Stdio};

use anyhow::anyhow;
use tracing::{debug, error, trace};
use update_informer::{registry, Check};

use crate::app::VersionStatus;

#[derive(Clone, Debug)]
pub struct Release {
    _name: String,
    assets: Vec<Asset>,
}

#[derive(Clone, Debug)]
pub struct Asset {
    name: String,
    download_url: String,
}

pub fn update() -> Result<(), Box<dyn std::error::Error>> {
    // get the first available release
    let release = get_latest_release("printer_client")?;
    trace!("{:#?}", release);
    debug!("Got latest release");

    let mut installer = None;
    let mut file_type = None;

    let files = release
        .assets
        .into_iter()
        .filter(|val| val.name.contains("msi") || val.name.contains("sh"))
        .filter(|val| !val.name.contains("sha"));

    // If the OS matches, installer will be set to it (Compiler flags will dictate this)
    for file in files {
        #[cfg(target_os = "windows")]
        if file.name.contains("msi") {
            installer = Some(file);
            file_type = Some("msi");
        }

        #[cfg(target_os = "linux")]
        if file.name.contains("sh") {
            installer = Some(file);
            file_type = Some("sh")
        }

        // plan to add this later
        #[cfg(target_os = "macos")]
        {
            installer = None;
            file_type = None;
        }
    }

    let installer = match installer {
        Some(val) => val,
        None => {
            return Err(anyhow!(
            "No installer for supported OS; Check releases to see if your platform is supported"
        )
            .into())
        }
    };
    println!("[Installer]: {}", installer.name);

    let mut installed_update = tempfile::Builder::new()
        .prefix("printer_client")
        .suffix(&format!(".{}", file_type.unwrap()))
        .tempfile()?;
    debug!("Temporary file created");
    let mut response = reqwest::blocking::get(installer.download_url)?;
    debug!("Fetched response");

    // Directly read response to
    std::io::copy(&mut response, &mut installed_update)?;
    debug!("Copied file to file");

    if !response.status().is_success() {
        return Err(anyhow!("Failed to download update installer").into());
    }

    // Handle the installer to run
    if cfg!(target_os = "windows") {
        Command::new("msiexec")
            .arg("/i")
            .arg(installed_update.path())
            .spawn()?;

        process::exit(0);
    } else if cfg!(target_os = "linux") {
        Command::new("sh")
            .arg(installed_update.path())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        process::exit(0)
    }

    Ok(())
}

pub fn get_latest_release(name: &str) -> Result<Release, Box<dyn std::error::Error>> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases",
        "CodedMasonry", "remote_print"
    );
    let resp = reqwest::blocking::Client::new()
        .get(url)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "remote_print")
        .send()?;
    debug!("Got update API response");

    let binding = resp.json::<serde_json::Value>()?;
    let releases = match binding.as_array() {
        Some(val) => val,
        None => return Err(anyhow!("No Releases found").into()),
    };

    let release: &serde_json::Value = releases
        .into_iter()
        .filter(|val| val["name"].as_str().unwrap().contains(name))
        .next()
        .unwrap();

    return Ok(Release {
        _name: release["name"].to_string().replace("\"", ""),
        assets: parse_assets(release["assets"].as_array().unwrap()),
    });
}

fn parse_assets(assets: &[serde_json::Value]) -> Vec<Asset> {
    let mut result = Vec::new();
    for asset in assets {
        result.push(Asset {
            name: asset["name"].to_string().replace("\"", ""),
            download_url: asset["url"].to_string().replace("\"", ""),
        })
    }

    return result;
}

pub fn check_oudated() -> Result<VersionStatus, Box<dyn std::error::Error>> {
    let informer = update_informer::new(
        registry::Crates,
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
    );

    let status = match informer.check_version() {
        Ok(ver) => ver,
        Err(e) => {
            error!("Failed to fetch version status: {:#?}", e);
            return Err(e);
        }
    };

    if let Some(ver) = status {
        return Ok(VersionStatus::OutDated(ver.to_string()));
    } else {
        println!("Up To Date");
        return Ok(VersionStatus::UpToDate);
    }
}
