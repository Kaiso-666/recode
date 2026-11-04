//! Self-update from GitHub releases.
//!
//! The release workflow publishes one raw binary per platform plus a
//! `.sha256` sidecar, so an upgrade is a download, a checksum check, and an
//! atomic-enough replacement of the running executable. On Windows the running
//! image is renamed aside first; the next start removes it.

use crate::cli::UpgradeArgs;
use anyhow::Context;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub async fn run(args: UpgradeArgs) -> anyhow::Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let api =
        std::env::var("RECODE_UPGRADE_API").unwrap_or_else(|_| "https://api.github.com".into());
    let url = match &args.tag {
        Some(tag) => format!("{api}/repos/{}/releases/tags/{tag}", args.repo),
        None => format!("{api}/repos/{}/releases/latest", args.repo),
    };
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(format!("recode/{current}"))
        .build()?;
    let release: Release = http
        .get(&url)
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("cannot reach the release API: {error}"))?
        .error_for_status()
        .map_err(|error| anyhow::anyhow!("release lookup failed: {error}"))?
        .json()
        .await
        .context("invalid release JSON")?;
    let latest = release.tag_name.trim_start_matches('v').to_owned();
    let newer = is_newer(&latest, current);
    if !newer && args.tag.is_none() {
        println!("recode {current} is up to date");
        return Ok(());
    }
    if latest == current {
        println!("recode {current} is already installed");
        return Ok(());
    }
    if args.check {
        println!("update available: {current} -> {latest}");
        return Ok(());
    }
    if !args.yes {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("pass --yes to upgrade without a terminal");
        }
        print!("Install recode {latest} over {current}? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
            println!("cancelled");
            return Ok(());
        }
    }

    let asset_name =
        asset_name().ok_or_else(|| anyhow::anyhow!("no prebuilt binary for this platform"))?;
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "release {latest} has no asset named {asset_name} (found: {})",
                release
                    .assets
                    .iter()
                    .map(|asset| asset.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let bytes = download(&http, &asset.browser_download_url).await?;
    let checksum_url = format!("{}.sha256", asset.browser_download_url);
    let checksum = download_text(&http, &checksum_url)
        .await
        .with_context(|| format!("release {latest} has no {asset_name}.sha256 checksum"))?;
    let expected = checksum
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let actual = hex::encode(Sha256::digest(&bytes));
    if expected.len() != 64 || expected != actual {
        anyhow::bail!("checksum mismatch: expected {expected}, got {actual}");
    }

    let path = install(&bytes)?;
    println!("installed recode {latest} at {}", path.display());
    println!("restart recode to use the new version");
    Ok(())
}

async fn download(http: &reqwest::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    let response = http
        .get(url)
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("download failed: {error}"))?
        .error_for_status()
        .map_err(|error| anyhow::anyhow!("download failed: {error}"))?;
    Ok(response.bytes().await?.to_vec())
}

async fn download_text(http: &reqwest::Client, url: &str) -> anyhow::Result<String> {
    let response = http.get(url).send().await?.error_for_status()?;
    Ok(response.text().await?)
}

/// The asset naming convention used by `.github/workflows/release.yml`.
pub fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("recode-windows-x86_64.exe"),
        ("linux", "x86_64") => Some("recode-linux-x86_64"),
        ("macos", "aarch64") => Some("recode-macos-aarch64"),
        ("macos", "x86_64") => Some("recode-macos-x86_64"),
        _ => None,
    }
}

/// Compare dotted versions numerically. Prerelease/build metadata is ignored,
/// which is enough for the `vX.Y.Z` tags this repository publishes.
pub fn is_newer(latest: &str, current: &str) -> bool {
    fn parts(version: &str) -> [u64; 3] {
        let mut out = [0u64; 3];
        for (index, piece) in version
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .take(3)
            .enumerate()
        {
            out[index] = piece.parse().unwrap_or(0);
        }
        out
    }
    parts(latest) > parts(current)
}

fn install(bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot locate the running executable")?;
    let name = exe
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "recode".to_owned());
    let staged = exe.with_file_name(format!("{name}.new"));
    std::fs::write(&staged, bytes).with_context(|| format!("cannot write {}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    {
        let backup = exe.with_extension("old");
        let _ = std::fs::remove_file(&backup);
        std::fs::rename(&exe, &backup).with_context(|| {
            format!(
                "cannot move the running binary aside; replace {} manually",
                exe.display()
            )
        })?;
        if let Err(error) = std::fs::rename(&staged, &exe) {
            let _ = std::fs::rename(&backup, &exe);
            let _ = std::fs::remove_file(&staged);
            return Err(anyhow::anyhow!(
                "cannot install the new binary at {}: {error}; the old binary was restored",
                exe.display()
            ));
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(&staged, &exe)
            .with_context(|| format!("cannot replace {}", exe.display()))?;
    }
    Ok(exe)
}

/// Remove the previous executable left behind by a Windows upgrade.
pub fn cleanup_previous() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let backup = exe.with_extension("old");
    if backup.is_file() {
        let _ = std::fs::remove_file(backup);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_dotted_versions() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("1.0.10", "1.0.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
        assert!(!is_newer("1.0.0-beta.1", "1.0.0"));
    }

    #[test]
    fn knows_the_asset_for_the_host() {
        // The test hosts are the platforms the release workflow builds for.
        if matches!(
            (std::env::consts::OS, std::env::consts::ARCH),
            ("windows", "x86_64")
                | ("linux", "x86_64")
                | ("macos", "aarch64")
                | ("macos", "x86_64")
        ) {
            assert!(asset_name().is_some());
        }
    }
}
