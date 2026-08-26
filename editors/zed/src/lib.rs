use std::fs;

use zed::LanguageServerId;
use zed_extension_api::{self as zed, settings::LspSettings, Result};

const REPO: &str = "jolars/badness";

struct BadnessBinary {
    path: String,
    args: Option<Vec<String>>,
}

struct BadnessExtension {
    cached_binary_path: Option<String>,
}

#[derive(Debug, PartialEq)]
struct GithubReleaseDetails {
    asset_names: Vec<String>,
    downloaded_file_type: zed::DownloadedFileType,
    downloaded_directory: String,
    downloaded_binary_path: String,
}

impl BadnessExtension {
    fn language_server_binary(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<BadnessBinary> {
        let binary_settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .ok()
            .and_then(|settings| settings.binary);
        let binary_args = binary_settings
            .as_ref()
            .and_then(|settings| settings.arguments.clone());

        if let Some(path) = binary_settings.and_then(|settings| settings.path) {
            return Ok(BadnessBinary {
                path,
                args: binary_args,
            });
        }

        // Prefer an installed binary so distributions such as NixOS can use a
        // build tailored to the host instead of the downloaded glibc binary.
        if let Some(path) = worktree.which("badness") {
            return Ok(BadnessBinary {
                path,
                args: binary_args,
            });
        }

        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
                return Ok(BadnessBinary {
                    path: path.clone(),
                    args: binary_args,
                });
            }
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let release = zed::latest_github_release(
            REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;
        let version = release
            .version
            .strip_prefix('v')
            .unwrap_or(&release.version)
            .to_string();
        let (platform, architecture) = zed::current_platform();
        let details = GithubReleaseDetails::new(platform, architecture, version)?;
        let asset = details
            .asset_names
            .iter()
            .find_map(|name| release.assets.iter().find(|asset| &asset.name == name))
            .ok_or_else(|| {
                format!(
                    "Badness release {} has no asset matching any of {:?}",
                    release.version, details.asset_names
                )
            })?;

        if !fs::metadata(&details.downloaded_binary_path).is_ok_and(|metadata| metadata.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::download_file(
                &asset.download_url,
                &details.downloaded_directory,
                details.downloaded_file_type,
            )
            .map_err(|error| format!("Failed to download Badness: {error}"))?;
        }

        self.cached_binary_path = Some(details.downloaded_binary_path.clone());
        Ok(BadnessBinary {
            path: details.downloaded_binary_path,
            args: binary_args,
        })
    }
}

impl GithubReleaseDetails {
    fn new(platform: zed::Os, architecture: zed::Architecture, version: String) -> Result<Self> {
        let architecture = match architecture {
            zed::Architecture::Aarch64 => "aarch64",
            zed::Architecture::X8664 => "x86_64",
            zed::Architecture::X86 => {
                return Err("Badness does not publish binaries for 32-bit x86".into())
            }
        };

        let asset_names = match platform {
            zed::Os::Mac => vec![format!("badness-{architecture}-apple-darwin.tar.gz")],
            zed::Os::Linux => vec![
                format!("badness-{architecture}-unknown-linux-gnu.tar.gz"),
                format!("badness-{architecture}-unknown-linux-musl.tar.gz"),
            ],
            zed::Os::Windows => vec![format!("badness-{architecture}-pc-windows-msvc.zip")],
        };
        let downloaded_file_type = match platform {
            zed::Os::Mac | zed::Os::Linux => zed::DownloadedFileType::GzipTar,
            zed::Os::Windows => zed::DownloadedFileType::Zip,
        };
        let downloaded_directory = format!("badness-{version}");
        let binary_name = match platform {
            zed::Os::Mac | zed::Os::Linux => "badness",
            zed::Os::Windows => "badness.exe",
        };
        let downloaded_binary_path = format!("{downloaded_directory}/{binary_name}");

        Ok(Self {
            asset_names,
            downloaded_file_type,
            downloaded_directory,
            downloaded_binary_path,
        })
    }
}

impl zed::Extension for BadnessExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary = self.language_server_binary(language_server_id, worktree)?;
        Ok(zed::Command {
            command: binary.path,
            args: binary.args.unwrap_or_else(|| vec!["lsp".into()]),
            env: vec![],
        })
    }

    fn language_server_initialization_options(
        &mut self,
        server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        let settings = LspSettings::for_worktree(server_id.as_ref(), worktree)
            .ok()
            .and_then(|settings| settings.initialization_options)
            .unwrap_or_default();
        Ok(Some(settings))
    }

    fn language_server_workspace_configuration(
        &mut self,
        server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        let settings = LspSettings::for_worktree(server_id.as_ref(), worktree)
            .ok()
            .and_then(|settings| settings.settings)
            .unwrap_or_default();
        Ok(Some(settings))
    }
}

zed::register_extension!(BadnessExtension);

#[cfg(test)]
mod tests {
    use super::GithubReleaseDetails;
    use zed_extension_api::{Architecture, DownloadedFileType, Os};

    #[test]
    fn resolves_macos_release() {
        assert_eq!(
            GithubReleaseDetails::new(Os::Mac, Architecture::Aarch64, "0.20.0".into()),
            Ok(GithubReleaseDetails {
                asset_names: vec!["badness-aarch64-apple-darwin.tar.gz".into()],
                downloaded_file_type: DownloadedFileType::GzipTar,
                downloaded_directory: "badness-0.20.0".into(),
                downloaded_binary_path: "badness-0.20.0/badness".into(),
            })
        );
    }

    #[test]
    fn resolves_linux_release_with_musl_fallback() {
        assert_eq!(
            GithubReleaseDetails::new(Os::Linux, Architecture::X8664, "0.20.0".into()),
            Ok(GithubReleaseDetails {
                asset_names: vec![
                    "badness-x86_64-unknown-linux-gnu.tar.gz".into(),
                    "badness-x86_64-unknown-linux-musl.tar.gz".into(),
                ],
                downloaded_file_type: DownloadedFileType::GzipTar,
                downloaded_directory: "badness-0.20.0".into(),
                downloaded_binary_path: "badness-0.20.0/badness".into(),
            })
        );
    }

    #[test]
    fn resolves_windows_release() {
        assert_eq!(
            GithubReleaseDetails::new(Os::Windows, Architecture::Aarch64, "0.20.0".into()),
            Ok(GithubReleaseDetails {
                asset_names: vec!["badness-aarch64-pc-windows-msvc.zip".into()],
                downloaded_file_type: DownloadedFileType::Zip,
                downloaded_directory: "badness-0.20.0".into(),
                downloaded_binary_path: "badness-0.20.0/badness.exe".into(),
            })
        );
    }

    #[test]
    fn rejects_32_bit_x86() {
        assert!(GithubReleaseDetails::new(Os::Linux, Architecture::X86, "0.20.0".into()).is_err());
    }
}
