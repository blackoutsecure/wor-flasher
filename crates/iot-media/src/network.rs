use crate::{MediaError, Package, Progress, Result, check_cancel, copy_verified};
use reqwest::blocking::Client;
use reqwest::header::{
    ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, HeaderMap, TRANSFER_ENCODING, USER_AGENT,
};
use std::fs::File;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub(crate) fn check_url(url: &str) -> Result<()> {
    if url.len() > 2048 || !url.bytes().all(|c| (33..=126).contains(&c)) {
        return Err(MediaError::invalid(
            "download URL contains invalid characters",
        ));
    }
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| MediaError::invalid("invalid official download URL"))?;
    let authority = url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""));
    if parsed.scheme() != "https"
        || !url
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        || url.contains('\\')
        || authority.is_some_and(|value| value.contains('@'))
        || !matches!(
            parsed.host_str(),
            Some("go.microsoft.com" | "software-download.microsoft.com")
        )
        || parsed.port().is_some_and(|port| port != 443)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(MediaError::invalid(
            "download and redirects require HTTPS on an approved Microsoft host",
        ));
    }
    Ok(())
}

/// Injectable transport. Production uses `MicrosoftHttps`, never desktop TLS/cache settings.
pub trait DownloadProvider {
    fn download(
        &self,
        url: &str,
        package: &Package,
        target: &mut File,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<()>;
}

#[derive(Default)]
pub struct MicrosoftHttps;

pub(crate) fn check_response(
    status: u16,
    url: &str,
    headers: &HeaderMap,
    expected_size: u64,
) -> Result<()> {
    check_url(url)?;
    if status != 200 {
        return Err(MediaError::acquisition(
            "official download did not return HTTP 200",
        ));
    }
    let lengths: Vec<_> = headers.get_all(CONTENT_LENGTH).iter().collect();
    let length = lengths.first().and_then(|value| value.to_str().ok());
    if lengths.len() != 1
        || length.is_none_or(|value| {
            value.is_empty()
                || value.len() > 12
                || !value.bytes().all(|c| c.is_ascii_digit())
                || value.parse::<u64>().ok() != Some(expected_size)
        })
    {
        return Err(MediaError::invalid(
            "official download Content-Length does not match its pinned size",
        ));
    }
    if headers.contains_key(TRANSFER_ENCODING)
        || headers
            .get_all(CONTENT_ENCODING)
            .iter()
            .any(|value| !value.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err(MediaError::invalid(
            "encoded/chunked official download is not supported",
        ));
    }
    Ok(())
}

impl DownloadProvider for MicrosoftHttps {
    fn download(
        &self,
        url: &str,
        package: &Package,
        target: &mut File,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<()> {
        check_url(url)?;
        check_cancel(cancel)?;
        let client = Client::builder()
            .https_only(true)
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 4 || check_url(attempt.url().as_str()).is_err() {
                    attempt.error("unapproved official download redirect")
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| {
                MediaError::acquisition("cannot initialize verified IoT HTTPS transport")
            })?;
        let mut response = client
            .get(url)
            .header(USER_AGENT, "WoR-Flasher-IoT-Media/1")
            .header(ACCEPT_ENCODING, "identity")
            .send()
            .map_err(|_| {
                MediaError::acquisition(
                    "official HTTPS request failed (TLS/network/redirect error)",
                )
            })?;
        check_response(
            response.status().as_u16(),
            response.url().as_str(),
            response.headers(),
            package.size,
        )?;
        // No reqwest error text is forwarded: its Display/source may include URL query tokens.
        copy_verified(
            &mut response,
            target,
            package,
            "Downloading and verifying official ISO",
            progress,
            cancel,
            Duration::from_secs(1200),
        )
        .map_err(|error| match error {
            MediaError::Io(_) => {
                MediaError::acquisition("official HTTPS body read or private package write failed")
            }
            other => other,
        })
    }
}
