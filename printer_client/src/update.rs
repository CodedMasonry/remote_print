use std::{
    fs,
    process::{self, Command, Stdio},
    time::Duration,
};

use anyhow::anyhow;
use semver::Version;
use tracing::{debug, error};
use update_informer::{registry, Check};
use uuid::Uuid;

use crate::app::VersionStatus;

#[derive(Clone, Debug)]
pub struct Release {
    version: String,
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
    debug!("Got latest release");

    let mut installer = None;

    let files = release
        .assets
        .into_iter()
        .filter(|val| val.name.contains("msi") || val.name.contains("sh"))
        .filter(|val| !val.name.contains("sha"));

    // If the OS matches, installer will be set to it (Compiler flags will dictate this)
    for file in files {
        if cfg!(target_os = "windows") && file.name.contains("msi") {
            installer = Some(file);
        } else if cfg!(target_os = "linux") && file.name.contains("sh") {
            installer = Some(file);
        } else {
            installer = None;
        }
    }

    let installer: Asset = match installer {
        Some(val) => val,
        None => {
            return Err(anyhow!(
            "No installer for supported OS; Check releases to see if your platform is supported"
        )
            .into())
        }
    };
    println!("[Installer]: {}", installer.download_url);

    let path = format!(
        "{}{}-printer_client.msi",
        std::env::temp_dir().to_string_lossy(),
        Uuid::new_v4()
    );
    debug!("Temporary file created");

    let response = reqwest::blocking::Client::new()
        .get(installer.download_url)
        .header("User-Agent", "remote_print")
        .send()?;
    let status = response.status();
    debug!("Fetched response");

    if !status.is_success() {
        return Err(anyhow!("Failed to download update installer: {}", status.as_str()).into());
    }

    // Handle the installer to run
    if cfg!(target_os = "windows") {
        println!("{:#?}", path);
        fs::write(path.clone(), response.bytes()?)?;
        println!("Copied installer to file");

        Command::new("msiexec").arg("/i").arg(path).spawn()?;

        process::exit(0);
    } else if cfg!(target_os = "linux") {
        fs::write(path.clone(), response.text()?)?;
        println!("Copied installer to file");

        Command::new("sh")
            .arg(path)
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

    let binding: serde_json::Value = resp.json::<serde_json::Value>()?;
    let releases = match binding.as_array() {
        Some(val) => val,
        None => return Err(anyhow!("No Releases found").into()),
    };

    let mut formatted_releases = Vec::new();
    for rel in releases {
        if rel["tag_name"].to_string().contains(name) {
            let version = rel["tag_name"].to_string();

            formatted_releases.push(Release {
                assets: parse_assets(rel["assets"].as_array().unwrap()),
                version: version
                    .replace("\"", "")
                    .split("-v")
                    .last()
                    .unwrap()
                    .to_owned(),
            })
        }
    }

    let newest = find_most_recent(&formatted_releases)?;
    return Ok(newest.clone());
}

fn parse_assets(assets: &[serde_json::Value]) -> Vec<Asset> {
    let mut result = Vec::new();
    for asset in assets {
        result.push(Asset {
            name: asset["name"].to_string().replace("\"", ""),
            download_url: asset["browser_download_url"].to_string().replace("\"", ""),
        })
    }

    return result;
}

fn find_most_recent(items: &Vec<Release>) -> Result<&Release, Box<dyn std::error::Error>> {
    let mut parsed_versions = Vec::new();

    for item in items {
        match Version::parse(&item.version) {
            Ok(parsed_version) => {
                parsed_versions.push(parsed_version);
            }
            Err(e) => {
                return Err(anyhow!("Failed to parse [{:#?}]: {:#?}", item.version, e).into());
            }
        }
    }

    let most_recent = parsed_versions.iter().max();

    match most_recent {
        Some(most_recent) if most_recent > &Version::parse(env!("CARGO_PKG_VERSION")).unwrap() => {
            Ok(&items[parsed_versions
                .iter()
                .position(|v| v == most_recent)
                .unwrap()])
        }
        _ => Err(anyhow!("No version newer than current version").into()),
    }
}

pub fn check_oudated() -> Result<VersionStatus, Box<dyn std::error::Error>> {
    let informer = update_informer::new(
        registry::Crates,
        "printer_client",
        env!("CARGO_PKG_VERSION"),
    )
    .interval(Duration::ZERO);

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
