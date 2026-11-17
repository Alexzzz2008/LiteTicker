//! Verified native updates from the official v2.0.0+ stable release channel.
use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use reqwest::{Client, Response, Url, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::io::Write;
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

// shortcut: inspect the latest 30 releases; use a dedicated manifest if this channel outgrows that window.
const API: &str = "https://api.github.com/repos/Alexzzz2008/LiteTicker/releases?per_page=30";
const ASSET: &str = "LiteTicker.exe";
const CHECKSUM_ASSET: &str = "LiteTicker.exe.sha256";
const PRODUCT: &str = "LiteTicker";
const LIMIT: u64 = 20 * 1024 * 1024;
const METADATA_LIMIT: usize = 1024 * 1024;
const HELPER_ARG: &str = "--apply-native-update";
pub type Progress = Arc<dyn Fn(u8) + Send + Sync>;

#[derive(Debug)]
pub struct ChannelUnavailable;
impl fmt::Display for ChannelUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("尚未发布可用的 v2.0.0+ 正式更新，当前版本可继续使用。")
    }
}
impl std::error::Error for ChannelUnavailable {}
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub download_url: Url,
    pub sha256: String,
    pub size: u64,
    pub notes: String,
    pub page_url: Url,
}
#[derive(Clone)]
pub struct UpdateClient {
    http: Client,
    #[cfg(test)]
    origin: Option<String>,
    #[cfg(test)]
    temporary_root: Option<PathBuf>,
}
impl UpdateClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .user_agent(concat!("LiteTicker/", env!("CARGO_PKG_VERSION")))
                .redirect(Policy::none())
                .min_tls_version(reqwest::tls::Version::TLS_1_2)
                .build()?,
            #[cfg(test)]
            origin: None,
            #[cfg(test)]
            temporary_root: None,
        })
    }
    pub async fn check(&self, token: CancellationToken) -> Result<Option<Release>> {
        cancel(&token)?;
        let work = async {
            let response = self.request(API, "application/vnd.github+json").await?;
            if response.status() == 404 {
                return Err(ChannelUnavailable.into());
            }
            let response = response.error_for_status()?;
            let bytes = read_bounded(response, METADATA_LIMIT).await?;
            let releases: serde_json::Value =
                serde_json::from_slice(&bytes).context("正式版版本信息不是有效JSON")?;
            let mut latest = None;
            for value in releases.as_array().context("正式版版本列表格式无效")? {
                let Some(tag) = value["tag_name"].as_str() else {
                    bail!("版本标签字段缺失");
                };
                let Some(name) = tag.strip_prefix('v') else {
                    continue;
                };
                let Ok(candidate_version) = version(name) else {
                    continue;
                };
                let draft = value["draft"].as_bool().context("版本draft标记无效")?;
                let prerelease = value["prerelease"]
                    .as_bool()
                    .context("版本prerelease标记无效")?;
                if draft || prerelease {
                    continue;
                }
                if latest
                    .as_ref()
                    .is_none_or(|(old, _)| candidate_version > *old)
                {
                    latest = Some((candidate_version, value));
                }
            }
            let Some((_, metadata)) = latest else {
                return Err(ChannelUnavailable.into());
            };
            let checksum = self.checksum_if_needed(metadata).await?;
            let release = parse_release(metadata, checksum.as_deref())?;
            Ok((version(&release.version)? > version(crate::VERSION)?).then_some(release))
        };
        bounded(&token, Duration::from_secs(10), "检查正式版更新超时", work).await
    }
    pub async fn download(
        &self,
        release: &Release,
        progress: Option<Progress>,
        token: CancellationToken,
    ) -> Result<DownloadedUpdate> {
        validate_release(release)?;
        ensure!(
            version(&release.version)? > version(crate::VERSION)?,
            "更新版本必须高于当前正式版"
        );
        cancel(&token)?;
        let work = async {
            let proxy = Url::parse(&format!("https://gh-proxy.com/{}", release.download_url))?;
            match tokio::time::timeout(
                Duration::from_secs(15),
                self.download_attempt(&proxy, release, progress.as_ref(), &token),
            )
            .await
            {
                Ok(Ok(download)) => return Ok(download),
                _ => cancel(&token)?,
            }
            self.download_attempt(&release.download_url, release, progress.as_ref(), &token)
                .await
        };
        bounded(&token, Duration::from_secs(120), "下载正式版更新超时", work).await
    }
    async fn request(&self, url: &str, accept: &str) -> Result<Response> {
        #[cfg(test)]
        let rewritten = self.origin.as_ref().map(|origin| {
            let url = Url::parse(url).unwrap();
            format!(
                "{origin}{}{}",
                url.path(),
                url.query().map(|q| format!("?{q}")).unwrap_or_default()
            )
        });
        #[cfg(test)]
        let url = rewritten.as_deref().unwrap_or(url);
        Ok(self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, accept)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await?)
    }
    async fn checksum_if_needed(&self, metadata: &serde_json::Value) -> Result<Option<String>> {
        let executable = selected_asset(metadata, ASSET)?;
        if !executable["digest"].is_null() {
            github_digest(&executable["digest"])?;
            return Ok(None);
        }
        let sidecar = selected_asset(metadata, CHECKSUM_ASSET)?;
        let size = sidecar["size"].as_u64().context("校验文件大小无效")?;
        ensure!((64..=256).contains(&size), "校验文件大小超出上限");
        let tag = metadata["tag_name"].as_str().context("版本标签缺失")?;
        version(tag.strip_prefix('v').context("不是正式发行标签")?)?;
        let url = Url::parse(
            sidecar["browser_download_url"]
                .as_str()
                .context("缺少官方校验文件地址")?,
        )?;
        validate_https(&url)?;
        ensure!(
            url.host_str() == Some("github.com")
                && url.query().is_none()
                && url.path()
                    == format!("/Alexzzz2008/LiteTicker/releases/download/{tag}/{CHECKSUM_ASSET}"),
            "校验文件不属于同一官方正式发行"
        );
        // The authority for a missing API digest must never come from the download proxy.
        let response = self.open_asset(&url, &url, false).await?;
        let body = read_bounded(response, 256).await?;
        ensure!(body.len() as u64 == size, "官方校验文件大小不匹配");
        Ok(Some(checksum_file(&body)?))
    }
    async fn open_asset(
        &self,
        initial: &Url,
        official: &Url,
        allow_proxy: bool,
    ) -> Result<Response> {
        let mut url = initial.clone();
        for redirects in 0..=3 {
            validate_hop(&url, official, allow_proxy)?;
            let response = self
                .request(url.as_str(), "application/octet-stream")
                .await?;
            if response.status().is_redirection() {
                ensure!(redirects < 3, "更新下载重定向过多");
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .context("更新重定向缺少地址")?
                    .to_str()?;
                url = url.join(location)?;
                validate_hop(&url, official, allow_proxy)?;
            } else {
                return Ok(response.error_for_status()?);
            }
        }
        bail!("更新下载重定向过多")
    }
    async fn download_attempt(
        &self,
        url: &Url,
        release: &Release,
        progress: Option<&Progress>,
        token: &CancellationToken,
    ) -> Result<DownloadedUpdate> {
        let mut response = self.open_asset(url, &release.download_url, true).await?;
        if let Some(length) = response.content_length() {
            ensure!(length == release.size, "更新下载大小与官方资产不一致");
        }
        let root = std::env::temp_dir();
        #[cfg(test)]
        let root = self.temporary_root.as_ref().unwrap_or(&root);
        let directory = OwnedDirectory::new_in(&root)?;
        let partial = directory.path.join("payload.partial");
        let path = directory.path.join(ASSET);
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&partial)
            .await?;
        let mut total = 0_u64;
        let mut sha = Sha256::new();
        let mut last = 0;
        if let Some(progress) = progress {
            progress(0);
        }
        while let Some(chunk) = response.chunk().await? {
            cancel(token)?;
            total = total
                .checked_add(chunk.len() as u64)
                .context("下载大小溢出")?;
            ensure!(
                total <= release.size && total <= LIMIT,
                "更新下载超出声明大小或20MiB上限"
            );
            file.write_all(&chunk).await?;
            sha.update(&chunk);
            let next = (total * 100 / release.size).min(99) as u8;
            if next != last {
                if let Some(progress) = progress {
                    progress(next);
                }
                last = next;
            }
        }
        ensure!(total == release.size, "更新下载不完整");
        ensure!(
            hex(&sha.finalize()) == release.sha256.to_ascii_lowercase(),
            "更新文件SHA256校验失败"
        );
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        cancel(token)?;
        verify_candidate(&partial, release)?;
        fs::rename(&partial, &path)?;
        cancel(token)?;
        if let Some(progress) = progress {
            progress(100);
        }
        cancel(token)?;
        Ok(DownloadedUpdate {
            _directory: directory,
            path,
            release: release.clone(),
        })
    }
}
#[derive(Debug)]
pub struct DownloadedUpdate {
    _directory: OwnedDirectory,
    path: PathBuf,
    release: Release,
}
impl DownloadedUpdate {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn release(&self) -> &Release {
        &self.release
    }
}
#[derive(Debug)]
struct OwnedDirectory {
    path: PathBuf,
    keep: bool,
}
impl OwnedDirectory {
    #[cfg(windows)]
    fn new() -> Result<Self> {
        // The helper gets its own canonical Win32 path after restarting. Record the
        // same path representation when the parent creates its handoff directory.
        Self::new_in(&std::env::temp_dir().canonicalize()?)
    }
    fn new_in(root: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..16 {
            let ticks = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
                ^ ((std::process::id() as u128) << 64)
                ^ NEXT.fetch_add(1, Ordering::Relaxed) as u128;
            let path = root.join(format!("LiteTicker-native-update-{ticks:032x}"));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path, keep: false }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        bail!("无法创建正式版更新暂存目录")
    }
}
impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        if !self.keep && plain_directory(&self.path) {
            for name in [
                ASSET,
                "payload.partial",
                "payload.exe",
                "apply.exe",
                "plan.json",
                "result.txt",
            ] {
                let _ = fs::remove_file(self.path.join(name));
            }
            let _ = fs::remove_dir(&self.path);
        }
    }
}
async fn read_bounded(mut response: Response, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        response.content_length().is_none_or(|n| n <= limit as u64),
        "更新元数据超过上限"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(chunk.len() <= limit - bytes.len(), "更新元数据超过上限");
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn cancel(token: &CancellationToken) -> Result<()> {
    ensure!(!token.is_cancelled(), "更新请求已取消");
    Ok(())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
#[cfg(windows)]
fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut sha = Sha256::new();
    let mut buffer = [0; 81920];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        sha.update(&buffer[..n]);
    }
    Ok(hex(&sha.finalize()))
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct ReleaseVersion(u16, u16, u16);
fn version(value: &str) -> Result<ReleaseVersion> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let capture = RE
        .get_or_init(|| Regex::new(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$").unwrap())
        .captures(value)
        .context("正式版本必须是三段版本号，例如2.0.0")?;
    let parsed = ReleaseVersion(
        capture[1].parse()?,
        capture[2].parse()?,
        capture[3].parse()?,
    );
    ensure!(parsed.0 >= 2, "正式原生更新仅接受v2.0.0及以后版本");
    Ok(parsed)
}
fn release_tag(release: &Release) -> String {
    format!("v{}", release.version)
}
fn validate_https(url: &Url) -> Result<()> {
    ensure!(
        url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "更新仅允许无用户信息的HTTPS地址"
    );
    Ok(())
}
fn validate_release(release: &Release) -> Result<()> {
    version(&release.version)?;
    ensure!(
        release.size > 0 && release.size <= LIMIT,
        "更新资产必须大于0且不超过20MiB"
    );
    ensure!(
        release.sha256.len() == 64 && release.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
        "更新缺少有效SHA256"
    );
    let tag = release_tag(release);
    for (url, suffix) in [
        (&release.download_url, format!("download/{tag}/{ASSET}")),
        (&release.page_url, format!("tag/{tag}")),
    ] {
        validate_https(url)?;
        ensure!(
            url.host_str() == Some("github.com")
                && url.query().is_none()
                && url.path() == format!("/Alexzzz2008/LiteTicker/releases/{suffix}"),
            "更新地址不属于正式版资产"
        );
    }
    Ok(())
}
fn validate_hop(url: &Url, official: &Url, allow_proxy: bool) -> Result<()> {
    validate_https(url)?;
    match url.host_str() {
        Some("gh-proxy.com") => ensure!(
            allow_proxy && url.as_str() == format!("https://gh-proxy.com/{official}"),
            "更新代理地址不属于固定官方资产"
        ),
        Some("github.com") => ensure!(url == official, "更新重定向改变了官方资产"),
        Some(
            "release-assets.githubusercontent.com"
            | "objects.githubusercontent.com"
            | "github-releases.githubusercontent.com",
        ) => {}
        _ => bail!("更新被重定向到不受信任主机"),
    };
    Ok(())
}
fn selected_asset<'a>(
    metadata: &'a serde_json::Value,
    name: &str,
) -> Result<&'a serde_json::Value> {
    let assets = metadata["assets"]
        .as_array()
        .context("版本信息缺少发布资产")?;
    let matches = assets
        .iter()
        .filter(|a| a["name"] == name)
        .collect::<Vec<_>>();
    ensure!(matches.len() == 1, "发布资产{name}缺失或重复");
    ensure!(
        matches[0]["state"] == "uploaded",
        "发布资产{name}尚未上传完成"
    );
    Ok(matches[0])
}
fn github_digest(value: &serde_json::Value) -> Result<String> {
    let hash = value
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .context("官方资产缺少有效SHA256摘要")?;
    ensure!(
        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "官方资产SHA256摘要格式无效"
    );
    Ok(hash.to_ascii_lowercase())
}
fn checksum_file(body: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(body).context("校验文件编码无效")?;
    static CHECKSUM: OnceLock<Regex> = OnceLock::new();
    let captures = CHECKSUM
        .get_or_init(|| {
            Regex::new(r"\A([0-9a-fA-F]{64})(?:[ \t]+\*?LiteTicker\.exe)?(?:\r?\n)?\z").unwrap()
        })
        .captures(text)
        .context("校验文件必须为单条LiteTicker.exe SHA256")?;
    Ok(captures[1].to_ascii_lowercase())
}
fn parse_release(value: &serde_json::Value, checksum: Option<&str>) -> Result<Release> {
    let tag = value["tag_name"].as_str().context("版本标签缺失")?;
    let name = tag.strip_prefix('v').context("不是正式发行标签")?;
    version(name)?;
    ensure!(
        value["draft"] == false && value["prerelease"] == false,
        "正式更新不能使用草稿或预发行版本"
    );
    let asset = selected_asset(value, ASSET)?;
    let digest = if asset["digest"].is_null() {
        checksum
            .context("官方资产缺少SHA256摘要与校验文件")?
            .to_owned()
    } else {
        github_digest(&asset["digest"])?
    };
    let release = Release {
        version: name.into(),
        download_url: Url::parse(
            asset["browser_download_url"]
                .as_str()
                .context("缺少下载地址")?,
        )?,
        sha256: digest.to_ascii_lowercase(),
        size: asset["size"].as_u64().context("资产大小无效")?,
        notes: match &value["body"] {
            serde_json::Value::Null => String::new(),
            serde_json::Value::String(s) => s.clone(),
            _ => bail!("版本说明格式无效"),
        },
        page_url: Url::parse(value["html_url"].as_str().context("缺少发布页地址")?)?,
    };
    validate_release(&release)?;
    Ok(release)
}

#[derive(Debug)]
struct NativeIdentity {
    version: ReleaseVersion,
}
fn verify_candidate(path: &Path, release: &Release) -> Result<NativeIdentity> {
    validate_release(release)?;
    let data = read_file_bounded(path, LIMIT)?;
    ensure!(data.len() as u64 == release.size, "更新文件大小不匹配");
    ensure!(
        hex(&Sha256::digest(&data)) == release.sha256.to_ascii_lowercase(),
        "更新文件SHA256不匹配"
    );
    let identity = native_identity(&data)?;
    ensure!(
        identity.version == version(&release.version)?,
        "更新产品版本与发布版本不匹配"
    );
    Ok(identity)
}
fn regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "更新路径必须是普通文件"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "更新路径不可为重解析点"
        );
    }
    Ok(())
}
fn bytes(data: &[u8], offset: usize, len: usize) -> Result<&[u8]> {
    data.get(offset..offset.checked_add(len).context("PE偏移溢出")?)
        .context("PE数据越界")
}
fn u16_at(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(data, offset, 2)?.try_into()?))
}
fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(data, offset, 4)?.try_into()?))
}
fn native_identity(data: &[u8]) -> Result<NativeIdentity> {
    ensure!(
        data.len() as u64 <= LIMIT && bytes(data, 0, 2)? == b"MZ",
        "不是受支持的原生EXE"
    );
    let pe = u32_at(data, 0x3c)? as usize;
    ensure!(bytes(data, pe, 4)? == b"PE\0\0", "PE签名无效");
    ensure!(
        u16_at(data, pe + 4)? == 0x8664,
        "更新必须是Windows x64原生程序"
    );
    let count = u16_at(data, pe + 6)? as usize;
    ensure!((1..=96).contains(&count), "PE节数量无效");
    let characteristics = u16_at(data, pe + 22)?;
    ensure!(
        characteristics & 2 != 0 && characteristics & 0x2000 == 0,
        "更新必须是EXE而非DLL"
    );
    let optional = pe + 24;
    let optional_size = u16_at(data, pe + 20)? as usize;
    ensure!(
        optional_size >= 240 && u16_at(data, optional)? == 0x20b,
        "更新必须是PE32+程序"
    );
    bytes(data, optional, optional_size)?;
    ensure!(u32_at(data, optional + 108)? >= 16, "PE数据目录不完整");
    ensure!(
        u32_at(data, optional + 224)? == 0 && u32_at(data, optional + 228)? == 0,
        "拒绝.NET托管程序集；更新只接受原生Windows EXE"
    );
    let sections = optional + optional_size;
    bytes(data, sections, count * 40)?;
    let locate = |rva: u32, size: usize| -> Result<&[u8]> {
        for index in 0..count {
            let section = sections + index * 40;
            let virtual_address = u32_at(data, section + 12)?;
            let raw_size = u32_at(data, section + 16)? as usize;
            let raw_offset = u32_at(data, section + 20)? as usize;
            if rva >= virtual_address {
                let delta = (rva - virtual_address) as usize;
                if delta <= raw_size && size <= raw_size - delta {
                    return bytes(data, raw_offset + delta, size);
                }
            }
        }
        bail!("PE资源地址不属于文件节")
    };
    let resource = locate(
        u32_at(data, optional + 128)?,
        u32_at(data, optional + 132)? as usize,
    )?;
    let entries = |offset: usize| -> Result<Vec<(u32, u32)>> {
        let named = u16_at(resource, offset + 12)? as usize;
        let ids = u16_at(resource, offset + 14)? as usize;
        ensure!(named + ids <= 4096, "PE资源目录过大");
        bytes(resource, offset + 16, (named + ids) * 8)?;
        (0..named + ids)
            .map(|i| {
                Ok((
                    u32_at(resource, offset + 16 + i * 8)?,
                    u32_at(resource, offset + 20 + i * 8)?,
                ))
            })
            .collect()
    };
    let versions = entries(0)?
        .into_iter()
        .filter(|(name, _)| *name == 16)
        .collect::<Vec<_>>();
    ensure!(
        versions.len() == 1 && versions[0].1 & 0x80000000 != 0,
        "缺少唯一版本资源"
    );
    let names = entries((versions[0].1 & 0x7fffffff) as usize)?;
    ensure!(
        names.len() == 1 && names[0].1 & 0x80000000 != 0,
        "版本资源名称不唯一"
    );
    let languages = entries((names[0].1 & 0x7fffffff) as usize)?;
    ensure!(
        languages.len() == 1 && languages[0].1 & 0x80000000 == 0,
        "版本资源语言不唯一"
    );
    let leaf = languages[0].1 as usize;
    let size = u32_at(resource, leaf + 4)? as usize;
    ensure!(size <= 65536, "版本资源过大");
    let version_data = locate(u32_at(resource, leaf)?, size)?;
    let mut strings = std::collections::HashMap::new();
    let fixed = parse_version_block(version_data, 0, 0, &mut strings)?;
    let fixed = fixed.context("缺少固定FileVersion")?;
    ensure!(
        strings.get("ProductName").map(String::as_str) == Some(PRODUCT),
        "更新产品不是正式版"
    );
    ensure!(
        strings.get("OriginalFilename").map(String::as_str) == Some(ASSET),
        "更新原始文件名不是正式版资产"
    );
    let version = version(
        strings
            .get("ProductVersion")
            .context("缺少正式版ProductVersion")?,
    )?;
    ensure!(
        fixed == [version.0, version.1, version.2, 0],
        "产品FileVersion不匹配"
    );
    Ok(NativeIdentity { version })
}
fn parse_version_block(
    data: &[u8],
    offset: usize,
    depth: usize,
    strings: &mut std::collections::HashMap<String, String>,
) -> Result<Option<[u16; 4]>> {
    ensure!(depth <= 8, "版本资源嵌套过深");
    let length = u16_at(data, offset)? as usize;
    ensure!(length >= 6, "版本资源长度无效");
    let block = bytes(data, offset, length)?;
    let value_length = u16_at(block, 2)? as usize;
    let kind = u16_at(block, 4)?;
    ensure!(kind <= 1, "版本资源类型无效");
    let mut cursor = 6;
    let mut key = Vec::new();
    loop {
        let c = u16_at(block, cursor)?;
        cursor += 2;
        if c == 0 {
            break;
        }
        key.push(c);
    }
    let key = String::from_utf16(&key)?;
    cursor = (cursor + 3) & !3;
    let value_bytes = if kind == 1 {
        value_length.checked_mul(2).context("版本字符串长度溢出")?
    } else {
        value_length
    };
    let value = bytes(block, cursor, value_bytes)?;
    let mut fixed = None;
    if depth == 0 {
        ensure!(
            key == "VS_VERSION_INFO" && value.len() >= 52 && u32_at(value, 0)? == 0xFEEF04BD,
            "固定版本资源无效"
        );
        let ms = u32_at(value, 8)?;
        let ls = u32_at(value, 12)?;
        fixed = Some([(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16]);
    }
    if kind == 1 && value_length > 0 {
        let utf16 = value
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>();
        let end = utf16.iter().position(|c| *c == 0).unwrap_or(utf16.len());
        let value = String::from_utf16(&utf16[..end])?;
        ensure!(strings.insert(key, value).is_none(), "重复的产品版本字段");
    }
    cursor = (cursor + value_bytes + 3) & !3;
    let mut children = 0;
    while cursor < block.len() {
        ensure!(children < 256, "版本资源子项过多");
        let child_length = u16_at(block, cursor)? as usize;
        ensure!(child_length >= 6, "版本资源子项长度无效");
        parse_version_block(block, cursor, depth + 1, strings)?;
        cursor = (cursor + child_length + 3) & !3;
        children += 1;
    }
    Ok(fixed)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Plan {
    target: PathBuf,
    payload: PathBuf,
    version: String,
    sha256: String,
    size: u64,
    original_sha256: String,
    original_version: String,
    parent_id: u32,
    parent_started: u64,
    wait_ms: u32,
}
fn plan_release(plan: &Plan) -> Result<Release> {
    let tag = format!("v{}", plan.version);
    Ok(Release {
        version: plan.version.clone(),
        sha256: plan.sha256.clone(),
        size: plan.size,
        notes: String::new(),
        download_url: Url::parse(&format!(
            "https://github.com/Alexzzz2008/LiteTicker/releases/download/{tag}/{ASSET}"
        ))?,
        page_url: Url::parse(&format!(
            "https://github.com/Alexzzz2008/LiteTicker/releases/tag/{tag}"
        ))?,
    })
}
/// Starts the copied helper. The caller exits only after this returns successfully.
pub fn launch_update(download: DownloadedUpdate) -> Result<()> {
    #[cfg(not(windows))]
    {
        let _ = download;
        bail!("正式版自动替换仅支持Windows");
    }
    #[cfg(windows)]
    {
        let target = std::env::current_exe()?.canonicalize()?;
        let original = native_identity(&fs::read(&target)?)?;
        ensure!(
            original.version == version(crate::VERSION)?,
            "当前运行程序的正式版身份或版本不匹配"
        );
        verify_candidate(&download.path(), download.release())?;
        ensure!(
            version(&download.release.version)? > original.version,
            "更新必须高于当前版本"
        );
        let mut directory = OwnedDirectory::new()?;
        let payload = directory.path.join("payload.exe");
        copy_synced(&download.path(), &payload)?;
        verify_candidate(&payload, download.release())?;
        let plan = Plan {
            target: target.clone(),
            payload,
            version: download.release.version.clone(),
            sha256: download.release.sha256.clone(),
            size: download.release.size,
            original_sha256: hash_file(&target)?,
            original_version: crate::VERSION.into(),
            parent_id: std::process::id(),
            parent_started: current_process_started()?,
            wait_ms: 20_000,
        };
        let helper = directory.path.join("apply.exe");
        copy_synced(&target, &helper)?;
        verify_old(&helper, &plan)?;
        let manifest = directory.path.join("plan.json");
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&manifest)?;
        file.write_all(&serde_json::to_vec(&plan)?)?;
        file.sync_all()?;
        drop(file);
        let digest = hash_file(&manifest)?;
        use std::os::windows::fs::OpenOptionsExt;
        let helper_guard = OpenOptions::new().read(true).share_mode(1).open(&helper)?;
        verify_old(&helper, &plan)?;
        let mut command = Command::new(&helper);
        command
            .arg(HELPER_ARG)
            .arg(&manifest)
            .arg(digest)
            .current_dir(&directory.path);
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
        command.spawn().context("无法启动原生更新助手")?;
        directory.keep = true;
        drop(helper_guard);
        Ok(())
    }
}
/// Invoke before single-instance acquisition or creating any windows.
pub fn run_helper_if_requested() -> Option<i32> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_none_or(|a| a != HELPER_ARG) {
        return None;
    }
    Some(match run_helper(&args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("正式版更新未完成：{error:#}");
            #[cfg(windows)]
            {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    MB_ICONERROR, MB_OK, MessageBoxW,
                };
                let text = format!("正式版更新未完成：{error:#}")
                    .encode_utf16()
                    .chain(Some(0))
                    .collect::<Vec<_>>();
                let title = "轻盘正式版更新"
                    .encode_utf16()
                    .chain(Some(0))
                    .collect::<Vec<_>>();
                unsafe {
                    MessageBoxW(
                        std::ptr::null_mut(),
                        text.as_ptr(),
                        title.as_ptr(),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
            1
        }
    })
}
fn run_helper(args: &[std::ffi::OsString]) -> Result<()> {
    ensure!(args.len() == 3, "原生更新助手参数无效");
    let executable = std::env::current_exe()?.canonicalize()?;
    let directory = executable.parent().context("更新助手目录无效")?;
    ensure!(
        executable.file_name().is_some_and(|n| n == "apply.exe") && owned_container(directory),
        "不是自有更新助手目录"
    );
    let manifest = directory.join("plan.json");
    ensure!(
        Path::new(&args[1]).canonicalize()? == manifest.canonicalize()?,
        "更新清单路径无效"
    );
    let manifest_data = read_file_bounded(&manifest, 16384)?;
    let digest = args[2].to_str().context("更新清单摘要无效")?;
    ensure!(
        digest.len() == 64
            && digest.bytes().all(|b| b.is_ascii_hexdigit())
            && hex(&Sha256::digest(&manifest_data)) == digest.to_ascii_lowercase(),
        "更新清单SHA256不匹配"
    );
    let plan: Plan = serde_json::from_slice(&manifest_data)?;
    ensure!(
        plan.payload == directory.join("payload.exe")
            && plan.target.is_absolute()
            && plan.target.canonicalize()? != executable,
        "更新清单路径无效"
    );
    verify_old(&executable, &plan)?;
    let outcome = apply_plan(&plan, wait_for_parent, start_process);
    let message = match &outcome {
        Ok(()) => "更新完成，已启动正式版新版。".into(),
        Err(error) => format!("更新未完成：{error:#}"),
    };
    let _ = fs::write(directory.join("result.txt"), message);
    let _ = fs::remove_file(directory.join("payload.exe"));
    let _ = fs::remove_file(manifest);
    outcome
}
fn owned_container(directory: &Path) -> bool {
    let Some(name) = directory.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some(id) = name.strip_prefix("LiteTicker-native-update-") else {
        return false;
    };
    id.len() == 32
        && id.bytes().all(|b| b.is_ascii_hexdigit())
        && directory.parent().and_then(|p| p.canonicalize().ok())
            == std::env::temp_dir().canonicalize().ok()
        && plain_directory(directory)
}
fn verify_old(path: &Path, plan: &Plan) -> Result<()> {
    let data = read_file_bounded(path, LIMIT)?;
    ensure!(
        hex(&Sha256::digest(&data)) == plan.original_sha256,
        "原程序或备份已变化，更新已取消"
    );
    let old = native_identity(&data)?;
    ensure!(
        old.version == version(&plan.original_version)? && old.version == version(crate::VERSION)?,
        "原程序正式版身份或版本不匹配"
    );
    Ok(())
}
fn copy_synced(from: &Path, to: &Path) -> Result<()> {
    regular_file(from)?;
    let mut input = File::open(from)?;
    let mut output = OpenOptions::new().create_new(true).write(true).open(to)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(())
}
fn apply_plan(
    plan: &Plan,
    wait: impl FnOnce(&Plan) -> Result<()>,
    mut start: impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    ensure!(
        plan.target.is_absolute()
            && plan.payload.is_absolute()
            && plan.target != plan.payload
            && plan
                .target
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("exe")),
        "更新文件路径无效"
    );
    ensure!(
        plan.parent_id > 0 && plan.parent_started > 0 && (1..=30000).contains(&plan.wait_ms),
        "更新进程参数无效"
    );
    let _lock = UpdateLock::acquire(&plan.target)?;
    let release = plan_release(plan)?;
    verify_candidate(&plan.payload, &release)?;
    ensure!(
        version(&plan.version)? > version(&plan.original_version)?,
        "更新版本必须高于当前版本"
    );
    wait(plan)?;
    verify_old(&plan.target, plan)?;
    let parent = plan.target.parent().context("更新目标目录无效")?;
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let candidate = parent.join(format!(".LiteTicker-native-{unique:032x}.new"));
    let backup = plan.target.with_file_name(format!(
        "{}.previous",
        plan.target.file_name().unwrap().to_string_lossy()
    ));
    let mut replaced = false;
    let result = (|| -> Result<()> {
        copy_synced(&plan.payload, &candidate)?;
        verify_candidate(&candidate, &release)?;
        verify_old(&plan.target, plan)?;
        retry(|| atomic_replace(&candidate, &plan.target, &backup))?;
        replaced = true;
        verify_candidate(&plan.target, &release)?;
        start(&plan.target)?;
        Ok(())
    })();
    let _ = fs::remove_file(&candidate);
    if let Err(error) = result {
        if replaced || (!plan.target.exists() && backup.exists()) {
            let restored = (|| -> Result<()> {
                verify_old(&backup, plan)?;
                let rejected = parent.join(format!(".LiteTicker-native-{unique:032x}.rejected"));
                if plan.target.exists() {
                    retry(|| atomic_replace(&backup, &plan.target, &rejected))?;
                } else {
                    fs::rename(&backup, &plan.target)?;
                }
                let _ = fs::remove_file(rejected);
                verify_old(&plan.target, plan)?;
                start(&plan.target)?;
                Ok(())
            })();
            match restored {
                Ok(()) => bail!("更新失败，已恢复并启动旧版：{error:#}"),
                Err(restore) => bail!(
                    "更新失败：{error:#}；恢复或重启失败：{restore:#}；请保留备份 {}",
                    backup.display()
                ),
            }
        }
        bail!("更新失败，原程序未更改：{error:#}");
    }
    Ok(())
}
fn retry(mut action: impl FnMut() -> Result<()>) -> Result<()> {
    for attempt in 0..6 {
        match action() {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt == 5 {
                    return Err(error);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
    unreachable!()
}
fn start_process(path: &Path) -> Result<()> {
    let mut command = Command::new(path);
    command
        .arg("--after-native-update")
        .current_dir(path.parent().context("启动目录无效")?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let until = std::time::Instant::now() + Duration::from_millis(1500);
    while std::time::Instant::now() < until {
        if let Some(status) = child.try_wait()? {
            ensure!(status.success(), "正式版启动时异常退出");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}
#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
#[cfg(windows)]
fn atomic_replace(candidate: &Path, target: &Path, backup: &Path) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let (a, b, c) = (wide(target), wide(candidate), wide(backup));
    ensure!(
        unsafe {
            ReplaceFileW(
                a.as_ptr(),
                b.as_ptr(),
                c.as_ptr(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        } != 0,
        "原子替换失败：{}",
        std::io::Error::last_os_error()
    );
    Ok(())
}
#[cfg(not(windows))]
fn atomic_replace(candidate: &Path, target: &Path, backup: &Path) -> Result<()> {
    fs::copy(target, backup)?;
    File::open(backup)?.sync_all()?;
    fs::rename(candidate, target)?;
    Ok(())
}
#[cfg(windows)]
fn process_started(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<u64> {
    use windows_sys::Win32::{Foundation::FILETIME, System::Threading::GetProcessTimes};
    let (mut created, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    ensure!(
        unsafe { GetProcessTimes(handle, &mut created, &mut exit, &mut kernel, &mut user) } != 0,
        "无法核验更新进程时间"
    );
    Ok((created.dwHighDateTime as u64) << 32 | created.dwLowDateTime as u64)
}
#[cfg(windows)]
fn current_process_started() -> Result<u64> {
    process_started(unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() })
}
#[cfg(windows)]
fn wait_for_parent(plan: &Plan) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0},
        System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            QueryFullProcessImageNameW, WaitForSingleObject,
        },
    };
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            plan.parent_id,
        )
    };
    if handle.is_null() {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            return Ok(());
        }
        return Err(error.into());
    }
    let result = (|| -> Result<()> {
        ensure!(
            process_started(handle)? == plan.parent_started,
            "原进程已被其他进程替换"
        );
        if unsafe { WaitForSingleObject(handle, 0) } == WAIT_OBJECT_0 {
            return Ok(());
        }
        let mut buffer = vec![0_u16; 32768];
        let mut size = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) } == 0 {
            if unsafe { WaitForSingleObject(handle, 0) } == WAIT_OBJECT_0 {
                return Ok(());
            }
            bail!("无法核验原进程路径");
        }
        let path = PathBuf::from(String::from_utf16(&buffer[..size as usize])?);
        ensure!(
            path.canonicalize()? == plan.target.canonicalize()?,
            "原进程路径不匹配"
        );
        ensure!(
            unsafe { WaitForSingleObject(handle, plan.wait_ms) } == WAIT_OBJECT_0,
            "原程序仍在运行，更新已取消"
        );
        Ok(())
    })();
    unsafe {
        CloseHandle(handle);
    }
    result
}
#[cfg(not(windows))]
fn wait_for_parent(_plan: &Plan) -> Result<()> {
    bail!("更新助手仅支持Windows")
}
#[cfg(windows)]
struct UpdateLock(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl UpdateLock {
    fn acquire(target: &Path) -> Result<Self> {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0},
            System::Threading::WaitForSingleObject,
        };
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateMutexW(
                attributes: *const std::ffi::c_void,
                owned: i32,
                name: *const u16,
            ) -> windows_sys::Win32::Foundation::HANDLE;
        }
        let name = format!(
            "Local\\LiteTicker.Native.Update.{}",
            hex(&Sha256::digest(
                target.to_string_lossy().to_uppercase().as_bytes()
            ))
        );
        let name = name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        ensure!(!handle.is_null(), "无法建立更新锁");
        let result = unsafe { WaitForSingleObject(handle, 1000) };
        if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
            unsafe {
                CloseHandle(handle);
            }
            bail!("另一正式版更新正在处理此文件");
        }
        Ok(Self(handle))
    }
}
#[cfg(windows)]
impl Drop for UpdateLock {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.0);
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
#[cfg(not(windows))]
struct UpdateLock {
    _guard: std::sync::MutexGuard<'static, ()>,
}
#[cfg(not(windows))]
impl UpdateLock {
    fn acquire(_target: &Path) -> Result<Self> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        Ok(Self {
            _guard: LOCK.lock().map_err(|_| anyhow::anyhow!("更新锁不可用"))?,
        })
    }
}

fn read_file_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    regular_file(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let file = options.open(path)?;
    ensure!(file.metadata()?.len() <= limit, "更新文件超过大小上限");
    let mut data = Vec::new();
    file.take(limit + 1).read_to_end(&mut data)?;
    ensure!(data.len() as u64 <= limit, "更新文件超过大小上限");
    Ok(data)
}
#[cfg(test)]
#[path = "../tests/fixtures/update.rs"]
mod tests;

async fn bounded<T>(
    token: &CancellationToken,
    duration: Duration,
    message: &'static str,
    work: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    cancel(token)?;
    tokio::select! { biased;
        _ = token.cancelled() => bail!("更新请求已取消"),
        result = tokio::time::timeout(duration, work) => result.context(message)?,
    }
}

fn plain_directory(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}
