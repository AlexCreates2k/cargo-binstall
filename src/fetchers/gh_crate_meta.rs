use std::path::Path;
use std::sync::Arc;

use log::{debug, info, warn};
use reqwest::Client;
use reqwest::Method;
use serde::Serialize;
use url::Url;

use super::Data;
use crate::{download_and_extract, remote_exists, BinstallError, PkgFmt, Template};

pub struct GhCrateMeta {
    client: Client,
    data: Data,
}

impl GhCrateMeta {
    fn url(&self) -> Result<Url, BinstallError> {
        let ctx = Context::from_data(&self.data);
        debug!("Using context: {:?}", ctx);
        ctx.render_url(&self.data.meta.pkg_url)
    }
}

#[async_trait::async_trait]
impl super::Fetcher for GhCrateMeta {
    async fn new(client: &Client, data: &Data) -> Arc<Self> {
        Arc::new(Self {
            client: client.clone(),
            data: data.clone(),
        })
    }

    async fn check(&self) -> Result<bool, BinstallError> {
        let url = self.url()?;

        if url.scheme() != "https" {
            warn!(
                "URL is not HTTPS! This may become a hard error in the future, tell the upstream!"
            );
        }

        info!("Checking for package at: '{url}'");
        remote_exists(&self.client, url, Method::HEAD).await
    }

    async fn fetch_and_extract(&self, dst: &Path) -> Result<(), BinstallError> {
        let url = self.url()?;
        info!("Downloading package from: '{url}'");
        download_and_extract(&self.client, url, self.pkg_fmt(), dst).await
    }

    fn pkg_fmt(&self) -> PkgFmt {
        self.data.meta.pkg_fmt
    }

    fn source_name(&self) -> String {
        self.url()
            .map(|url| {
                if let Some(domain) = url.domain() {
                    domain.to_string()
                } else if let Some(host) = url.host_str() {
                    host.to_string()
                } else {
                    url.to_string()
                }
            })
            .unwrap_or_else(|_| "invalid url template".to_string())
    }

    fn is_third_party(&self) -> bool {
        false
    }

    fn target(&self) -> &str {
        &self.data.target
    }
}

/// Template for constructing download paths
#[derive(Clone, Debug, Serialize)]
struct Context<'c> {
    pub name: &'c str,
    pub repo: Option<&'c str>,
    pub target: &'c str,
    pub version: &'c str,

    /// Soft-deprecated alias for archive-format
    pub format: String,

    /// Archive format e.g. tar.gz, zip
    #[serde(rename = "archive-format")]
    pub archive_format: String,

    /// Filename extension on the binary, i.e. .exe on Windows, nothing otherwise
    #[serde(rename = "binary-ext")]
    pub binary_ext: &'c str,
}

impl<'c> Template for Context<'c> {}

impl<'c> Context<'c> {
    pub(self) fn from_data(data: &'c Data) -> Self {
        let pkg_fmt = data.meta.pkg_fmt.to_string();
        Self {
            name: &data.name,
            repo: data.repo.as_ref().map(|s| &s[..]),
            target: &data.target,
            version: &data.version,
            format: pkg_fmt.clone(),
            archive_format: pkg_fmt,
            binary_ext: if data.target.contains("windows") {
                ".exe"
            } else {
                ""
            },
        }
    }

    pub(self) fn render_url(&self, template: &str) -> Result<Url, BinstallError> {
        Ok(Url::parse(&self.render(template)?)?)
    }
}

#[cfg(test)]
mod test {
    use super::{super::Data, Context};
    use crate::{PkgFmt, PkgMeta};
    use url::Url;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn defaults() {
        let meta = PkgMeta::default();
        let data = Data {
            name: "cargo-binstall".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            version: "1.2.3".to_string(),
            repo: Some("https://github.com/ryankurte/cargo-binstall".to_string()),
            meta,
        };

        let ctx = Context::from_data(&data);
        assert_eq!(
            ctx.render_url(&data.meta.pkg_url).unwrap(),
            url("https://github.com/ryankurte/cargo-binstall/releases/download/v1.2.3/cargo-binstall-x86_64-unknown-linux-gnu-v1.2.3.tgz")
        );
    }

    #[test]
    #[should_panic]
    fn no_repo() {
        let meta = PkgMeta::default();
        let data = Data {
            name: "cargo-binstall".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            version: "1.2.3".to_string(),
            repo: None,
            meta,
        };

        let ctx = Context::from_data(&data);
        ctx.render_url(&data.meta.pkg_url).unwrap();
    }

    #[test]
    fn no_repo_but_full_url() {
        let meta = PkgMeta {
            pkg_url: format!("https://example.com{}", PkgMeta::default().pkg_url),
            ..Default::default()
        };

        let data = Data {
            name: "cargo-binstall".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            version: "1.2.3".to_string(),
            repo: None,
            meta,
        };

        let ctx = Context::from_data(&data);
        assert_eq!(
            ctx.render_url(&data.meta.pkg_url).unwrap(),
            url("https://example.com/releases/download/v1.2.3/cargo-binstall-x86_64-unknown-linux-gnu-v1.2.3.tgz")
        );
    }

    #[test]
    fn different_url() {
        let meta = PkgMeta {
            pkg_url:
            "{ repo }/releases/download/v{ version }/sx128x-util-{ target }-v{ version }.{ archive-format }"
                .into(),
            ..Default::default()
        };

        let data = Data {
            name: "radio-sx128x".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            version: "0.14.1-alpha.5".to_string(),
            repo: Some("https://github.com/rust-iot/rust-radio-sx128x".to_string()),
            meta,
        };

        let ctx = Context::from_data(&data);
        assert_eq!(
            ctx.render_url(&data.meta.pkg_url).unwrap(),
            url("https://github.com/rust-iot/rust-radio-sx128x/releases/download/v0.14.1-alpha.5/sx128x-util-x86_64-unknown-linux-gnu-v0.14.1-alpha.5.tgz")
        );
    }

    #[test]
    fn deprecated_format() {
        let meta = PkgMeta {
            pkg_url: "{ repo }/releases/download/v{ version }/sx128x-util-{ target }-v{ version }.{ format }".into(),
            ..Default::default()
        };

        let data = Data {
            name: "radio-sx128x".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            version: "0.14.1-alpha.5".to_string(),
            repo: Some("https://github.com/rust-iot/rust-radio-sx128x".to_string()),
            meta,
        };

        let ctx = Context::from_data(&data);
        assert_eq!(
            ctx.render_url(&data.meta.pkg_url).unwrap(),
            url("https://github.com/rust-iot/rust-radio-sx128x/releases/download/v0.14.1-alpha.5/sx128x-util-x86_64-unknown-linux-gnu-v0.14.1-alpha.5.tgz")
        );
    }

    #[test]
    fn different_ext() {
        let meta = PkgMeta {
            pkg_url:
                "{ repo }/releases/download/v{ version }/{ name }-v{ version }-{ target }.tar.xz"
                    .into(),
            pkg_fmt: PkgFmt::Txz,
            ..Default::default()
        };

        let data = Data {
            name: "cargo-watch".to_string(),
            target: "aarch64-apple-darwin".to_string(),
            version: "9.0.0".to_string(),
            repo: Some("https://github.com/watchexec/cargo-watch".to_string()),
            meta,
        };

        let ctx = Context::from_data(&data);
        assert_eq!(
            ctx.render_url(&data.meta.pkg_url).unwrap(),
            url("https://github.com/watchexec/cargo-watch/releases/download/v9.0.0/cargo-watch-v9.0.0-aarch64-apple-darwin.tar.xz")
        );
    }

    #[test]
    fn no_archive() {
        let meta = PkgMeta {
            pkg_url: "{ repo }/releases/download/v{ version }/{ name }-v{ version }-{ target }{ binary-ext }".into(),
            pkg_fmt: PkgFmt::Bin,
            ..Default::default()
        };

        let data = Data {
            name: "cargo-watch".to_string(),
            target: "aarch64-pc-windows-msvc".to_string(),
            version: "9.0.0".to_string(),
            repo: Some("https://github.com/watchexec/cargo-watch".to_string()),
            meta,
        };

        let ctx = Context::from_data(&data);
        assert_eq!(
            ctx.render_url(&data.meta.pkg_url).unwrap(),
            url("https://github.com/watchexec/cargo-watch/releases/download/v9.0.0/cargo-watch-v9.0.0-aarch64-pc-windows-msvc.exe")
        );
    }
}
