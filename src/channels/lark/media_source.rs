use anyhow::{bail, Context, Result};
use directories::UserDirs;
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NormalizedMediaSource {
    LocalPath(PathBuf),
    RemoteUrl(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DownloadedRemoteMedia {
    pub(crate) bytes: Vec<u8>,
    pub(crate) content_type: Option<String>,
    pub(crate) file_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalPathPolicy {
    DefaultRoots,
    ExplicitPathAllowed,
    SandboxValidated,
    AnyRootsWithReader,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteMediaPolicy {
    pub(crate) block_private_ips: bool,
    pub(crate) block_link_local_ips: bool,
    pub(crate) block_loopback_ips: bool,
    pub(crate) block_unspecified_ips: bool,
    pub(crate) block_multicast_ips: bool,
    pub(crate) blocked_hostnames: Vec<String>,
    pub(crate) blocked_host_suffixes: Vec<String>,
}

pub(crate) struct OutboundMediaLoadOptions {
    pub(crate) max_bytes: usize,
    pub(crate) workspace_dir: Option<PathBuf>,
    pub(crate) local_roots: Option<Vec<PathBuf>>,
    pub(crate) local_path_policy: LocalPathPolicy,
    pub(crate) read_local_bytes: Option<Arc<dyn Fn(&Path) -> Result<Vec<u8>> + Send + Sync>>,
    pub(crate) remote_policy: RemoteMediaPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoadedOutboundMedia {
    LocalPath(PathBuf),
    Downloaded(DownloadedRemoteMedia),
    InMemory(LoadedLocalMedia),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedLocalMedia {
    pub(crate) path: PathBuf,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) fn parse_media_source_input(raw: &str) -> Result<NormalizedMediaSource> {
    let trimmed = strip_media_prefix(raw.trim());
    let unwrapped = unwrap_media_source_wrappers(trimmed);
    if unwrapped.is_empty() {
        bail!("Invalid media input: empty value");
    }

    if let Ok(url) = reqwest::Url::parse(unwrapped) {
        return match url.scheme() {
            "http" | "https" => Ok(NormalizedMediaSource::RemoteUrl(url.to_string())),
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|_| anyhow::anyhow!("invalid file:// media path"))?;
                Ok(NormalizedMediaSource::LocalPath(path))
            }
            other => bail!("Unsupported media URL scheme: {other}"),
        };
    }

    let path = Path::new(unwrapped);
    if path.is_absolute() || looks_like_windows_absolute_path(unwrapped) {
        return Ok(NormalizedMediaSource::LocalPath(PathBuf::from(unwrapped)));
    }

    bail!("Invalid media input: expected an absolute local path, file:// URL, or http(s) URL")
}

pub(crate) fn validate_local_media_path(
    path: PathBuf,
    workspace_dir: Option<&Path>,
) -> Result<PathBuf> {
    let roots = default_media_local_roots(workspace_dir)?;
    validate_local_media_path_with_roots(path, &roots)
}

pub(crate) fn validate_explicit_local_media_path(
    path: PathBuf,
    workspace_dir: Option<&Path>,
) -> Result<PathBuf> {
    let canonical = canonicalize_media_file(&path)?;
    let mut roots = default_media_local_roots(workspace_dir)?;
    if let Some(parent) = canonical.parent() {
        roots.push(parent.to_path_buf());
    }
    normalize_roots(&mut roots)?;
    validate_local_media_path_with_roots(canonical, &roots)
}

pub(crate) fn default_media_local_roots(workspace_dir: Option<&Path>) -> Result<Vec<PathBuf>> {
    let mut roots = Vec::new();

    if let Some(workspace_dir) = workspace_dir {
        roots.push(canonical_or_resolve(workspace_dir)?);
    }

    roots.push(canonical_or_resolve(&std::env::temp_dir())?);

    if let Some(home_dir) = UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) {
        let zeroclaw_workspace = home_dir.join(".zeroclaw").join("workspace");
        roots.push(canonical_or_resolve(&zeroclaw_workspace)?);
    }

    normalize_roots(&mut roots)?;
    Ok(roots)
}

pub(crate) fn resolve_outbound_media_local_roots(
    media_local_roots: Option<Vec<PathBuf>>,
) -> Option<Vec<PathBuf>> {
    media_local_roots.filter(|roots| !roots.is_empty())
}

pub(crate) fn build_outbound_media_load_options(
    max_bytes: usize,
    workspace_dir: Option<&Path>,
    media_local_roots: Option<Vec<PathBuf>>,
    local_path_policy: LocalPathPolicy,
) -> OutboundMediaLoadOptions {
    OutboundMediaLoadOptions {
        max_bytes,
        workspace_dir: workspace_dir.map(Path::to_path_buf),
        local_roots: resolve_outbound_media_local_roots(media_local_roots),
        local_path_policy,
        read_local_bytes: None,
        remote_policy: default_remote_media_policy(),
    }
}

pub(crate) fn build_sandbox_validated_media_load_options(
    max_bytes: usize,
    sandbox_root: &Path,
    read_local_bytes: Arc<dyn Fn(&Path) -> Result<Vec<u8>> + Send + Sync>,
) -> OutboundMediaLoadOptions {
    let mut options = build_outbound_media_load_options(
        max_bytes,
        Some(sandbox_root),
        None,
        LocalPathPolicy::SandboxValidated,
    );
    options.read_local_bytes = Some(read_local_bytes);
    options
}

pub(crate) fn build_root_scoped_sandbox_media_load_options(
    max_bytes: usize,
    sandbox_root: &Path,
) -> OutboundMediaLoadOptions {
    build_sandbox_validated_media_load_options(
        max_bytes,
        sandbox_root,
        build_root_scoped_local_reader(sandbox_root),
    )
}

pub(crate) fn build_root_scoped_local_reader(
    root: &Path,
) -> Arc<dyn Fn(&Path) -> Result<Vec<u8>> + Send + Sync> {
    let root = canonical_or_resolve(root).unwrap_or_else(|_| root.to_path_buf());
    Arc::new(move |path: &Path| {
        let canonical = canonicalize_media_file(path)?;
        if !is_path_within(&canonical, &root) {
            bail!(
                "Local media path is not under an allowed directory: {}",
                canonical.display()
            );
        }
        Ok(std::fs::read(&canonical)?)
    })
}

pub(crate) fn validate_local_media_path_with_roots(
    path: PathBuf,
    roots: &[PathBuf],
) -> Result<PathBuf> {
    let canonical = canonicalize_media_file(&path)?;
    if roots.is_empty() || roots.iter().any(|root| is_path_within(&canonical, root)) {
        return Ok(canonical);
    }

    bail!(
        "Local media path is not under an allowed directory: {}",
        canonical.display()
    )
}

fn canonicalize_media_file(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("Invalid media input: empty local path");
    }
    if !path.is_absolute() {
        bail!("Invalid media input: local media path must be absolute");
    }

    let canonical = std::fs::canonicalize(&path)
        .with_context(|| format!("Local media path is not readable: {}", path.display()))?;
    let metadata = std::fs::metadata(&canonical)
        .with_context(|| format!("Failed to stat local media path: {}", canonical.display()))?;
    if !metadata.is_file() {
        bail!("Local media path is not a file: {}", canonical.display());
    }

    Ok(canonical)
}

pub(crate) fn default_remote_media_policy() -> RemoteMediaPolicy {
    RemoteMediaPolicy {
        block_private_ips: true,
        block_link_local_ips: true,
        block_loopback_ips: !cfg!(test),
        block_unspecified_ips: true,
        block_multicast_ips: true,
        blocked_hostnames: vec![
            "localhost".to_string(),
            "metadata.google.internal".to_string(),
        ],
        blocked_host_suffixes: vec![".local".to_string(), ".internal".to_string()],
    }
}

pub(crate) fn validate_remote_media_url(raw: &str) -> Result<String> {
    validate_remote_media_url_with_policy(raw, &default_remote_media_policy())
}

pub(crate) fn validate_remote_media_url_with_policy(
    raw: &str,
    policy: &RemoteMediaPolicy,
) -> Result<String> {
    let url = reqwest::Url::parse(raw).context("Invalid remote media URL")?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Remote media URL is missing host"))?;

    if policy
        .blocked_hostnames
        .iter()
        .any(|blocked| host.eq_ignore_ascii_case(blocked))
        || policy
            .blocked_host_suffixes
            .iter()
            .any(|suffix| host.ends_with(suffix))
    {
        bail!("Remote media URL points to a blocked host");
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip, policy) {
            bail!("Remote media URL points to a blocked IP");
        }
    }

    Ok(url.to_string())
}

pub(crate) async fn download_remote_media(
    client: &reqwest::Client,
    url: &str,
    max_bytes: usize,
) -> Result<DownloadedRemoteMedia> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("download remote media {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("download remote media failed with status {status}");
    }
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes as u64)
    {
        bail!("remote media exceeds size limit: more than {max_bytes} bytes");
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let file_name = response
        .headers()
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_content_disposition_file_name);
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("read remote media body {url}"))?
        .to_vec();
    if bytes.len() > max_bytes {
        bail!(
            "remote media exceeds size limit: {} bytes > {} bytes",
            bytes.len(),
            max_bytes
        );
    }

    Ok(DownloadedRemoteMedia {
        bytes,
        content_type,
        file_name,
    })
}

pub(crate) async fn load_outbound_media(
    client: &reqwest::Client,
    raw: &str,
    options: &OutboundMediaLoadOptions,
) -> Result<LoadedOutboundMedia> {
    match parse_media_source_input(raw)? {
        NormalizedMediaSource::LocalPath(path) => {
            let validated = match options.local_path_policy {
                LocalPathPolicy::DefaultRoots => {
                    if let Some(local_roots) = options.local_roots.as_ref() {
                        validate_local_media_path_with_roots(path, local_roots)?
                    } else {
                        validate_local_media_path(path, options.workspace_dir.as_deref())?
                    }
                }
                LocalPathPolicy::ExplicitPathAllowed => {
                    validate_explicit_local_media_path(path, options.workspace_dir.as_deref())?
                }
                LocalPathPolicy::SandboxValidated => canonicalize_media_file(&path)?,
                LocalPathPolicy::AnyRootsWithReader => {
                    if options.read_local_bytes.is_none() {
                        bail!(
                            "Refusing local roots bypass without read_local_bytes override"
                        );
                    }
                    canonicalize_media_file(&path)?
                }
            };
            if let Some(read_local_bytes) = options.read_local_bytes.as_ref() {
                let bytes = read_local_bytes(&validated)?;
                if bytes.len() > options.max_bytes {
                    bail!(
                        "local media exceeds size limit: {} bytes > {} bytes",
                        bytes.len(),
                        options.max_bytes
                    );
                }
                Ok(LoadedOutboundMedia::InMemory(LoadedLocalMedia {
                    path: validated,
                    bytes,
                }))
            } else {
                Ok(LoadedOutboundMedia::LocalPath(validated))
            }
        }
        NormalizedMediaSource::RemoteUrl(url) => {
            let normalized = validate_remote_media_url_with_policy(&url, &options.remote_policy)?;
            let downloaded = download_remote_media(client, &normalized, options.max_bytes).await?;
            Ok(LoadedOutboundMedia::Downloaded(downloaded))
        }
    }
}

fn is_blocked_ip(ip: IpAddr, policy: &RemoteMediaPolicy) -> bool {
    match ip {
        IpAddr::V4(ipv4) => is_blocked_ipv4(ipv4, policy),
        IpAddr::V6(ipv6) => is_blocked_ipv6(ipv6, policy),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr, policy: &RemoteMediaPolicy) -> bool {
    (policy.block_private_ips && ip.is_private())
        || (policy.block_loopback_ips && ip.is_loopback())
        || (policy.block_link_local_ips && ip.is_link_local())
        || (policy.block_multicast_ips && ip.is_multicast())
        || (policy.block_unspecified_ips && ip.is_unspecified())
        || ip.is_broadcast()
        || matches!(ip.octets(), [192, 0, 2, _] | [198, 51, 100, _] | [203, 0, 113, _])
}

fn is_blocked_ipv6(ip: Ipv6Addr, policy: &RemoteMediaPolicy) -> bool {
    (policy.block_loopback_ips && ip.is_loopback())
        || (policy.block_unspecified_ips && ip.is_unspecified())
        || (policy.block_multicast_ips && ip.is_multicast())
        || (policy.block_private_ips && ip.is_unique_local())
        || (policy.block_link_local_ips && ip.is_unicast_link_local())
        || matches!(ip.segments(), [0x2001, 0x0db8, ..])
}

fn normalize_roots(roots: &mut Vec<PathBuf>) -> Result<()> {
    for root in roots.iter_mut() {
        *root = canonical_or_resolve(root)?;
        if root == Path::new("/") {
            bail!("Invalid local media root: /");
        }
    }
    roots.sort();
    roots.dedup();
    Ok(())
}

fn canonical_or_resolve(path: &Path) -> Result<PathBuf> {
    Ok(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn is_path_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn unwrap_media_source_wrappers(raw: &str) -> &str {
    let mut value = raw.trim();

    if value.len() >= 2 && value.starts_with('<') && value.ends_with('>') {
        value = value[1..value.len() - 1].trim();
    }

    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if matches!((first, last), (b'"', b'"') | (b'\'', b'\'') | (b'`', b'`')) {
            value = value[1..value.len() - 1].trim();
        }
    }

    value
}

fn strip_media_prefix(raw: &str) -> &str {
    if let Some((prefix, rest)) = raw.split_once(':') {
        if prefix.trim().eq_ignore_ascii_case("MEDIA") {
            return rest.trim();
        }
    }
    raw
}

fn looks_like_windows_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes, [drive, b':', sep, ..] if drive.is_ascii_alphabetic() && (*sep == b'/' || *sep == b'\\'))
        || value.starts_with("\\\\")
}

fn parse_content_disposition_file_name(disposition: &str) -> Option<String> {
    let match_ = disposition
        .split(';')
        .find_map(|part| part.trim().strip_prefix("filename="))
        .or_else(|| {
            disposition
                .split(';')
                .find_map(|part| part.trim().strip_prefix("filename*=UTF-8''"))
        })?;
    let trimmed = match_.trim_matches('"').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_media_source_input_accepts_wrapped_remote_url() {
        let parsed = parse_media_source_input(" <\"https://example.com/demo.png?sig=1\"> ").unwrap();
        assert_eq!(
            parsed,
            NormalizedMediaSource::RemoteUrl("https://example.com/demo.png?sig=1".to_string())
        );
    }

    #[test]
    fn parse_media_source_input_accepts_file_url() {
        let parsed = parse_media_source_input("file:///tmp/demo.png").unwrap();
        assert_eq!(parsed, NormalizedMediaSource::LocalPath(PathBuf::from("/tmp/demo.png")));
    }

    #[test]
    fn parse_media_source_input_accepts_media_prefixed_file_url() {
        let parsed = parse_media_source_input("  MEDIA :  file:///tmp/demo.png ").unwrap();
        assert_eq!(parsed, NormalizedMediaSource::LocalPath(PathBuf::from("/tmp/demo.png")));
    }

    #[test]
    fn parse_media_source_input_rejects_relative_local_path() {
        let err = parse_media_source_input("./demo.png").unwrap_err();
        assert!(err.to_string().contains("expected an absolute local path"));
    }

    #[test]
    fn parse_media_source_input_rejects_unsupported_scheme() {
        let err = parse_media_source_input("ftp://example.com/demo.png").unwrap_err();
        assert!(err.to_string().contains("Unsupported media URL scheme"));
    }

    #[test]
    fn validate_local_media_path_allows_workspace_file() {
        let workspace = TempDir::new().unwrap();
        let file = workspace.path().join("demo.png");
        std::fs::write(&file, b"png").unwrap();

        let validated = validate_local_media_path(file, Some(workspace.path())).unwrap();
        assert!(validated.is_absolute());
    }

    #[test]
    fn validate_local_media_path_rejects_file_outside_allowed_roots() {
        let workspace = TempDir::new().unwrap();
        let repo_root = std::env::current_dir().unwrap();
        let outside = tempfile::tempdir_in(repo_root).unwrap();
        let file = outside.path().join("demo.png");
        std::fs::write(&file, b"png").unwrap();

        let err = validate_local_media_path(file, Some(workspace.path())).unwrap_err();
        assert!(err.to_string().contains("not under an allowed directory"));
    }

    #[test]
    fn validate_local_media_path_allows_temp_file_via_default_tmp_root() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("demo.png");
        std::fs::write(&file, b"png").unwrap();

        let validated = validate_local_media_path(file, None).unwrap();
        assert!(validated.is_absolute());
    }

    #[test]
    fn validate_explicit_local_media_path_allows_file_outside_default_roots() {
        let repo_root = std::env::current_dir().unwrap();
        let outside = tempfile::tempdir_in(repo_root).unwrap();
        let file = outside.path().join("demo.png");
        std::fs::write(&file, b"png").unwrap();

        let validated = validate_explicit_local_media_path(file, None).unwrap();
        assert!(validated.is_absolute());
    }

    #[test]
    fn validate_remote_media_url_rejects_private_ipv4_hosts() {
        let err = validate_remote_media_url("http://10.0.0.1/demo.png").unwrap_err();
        assert!(err.to_string().contains("blocked IP"));
    }

    #[test]
    fn validate_remote_media_url_rejects_ipv6_unique_local_hosts() {
        let err = validate_remote_media_url("http://[fd00::1]/demo.png").unwrap_err();
        assert!(err.to_string().contains("blocked IP"));
    }

    #[test]
    fn validate_remote_media_url_with_policy_allows_loopback_when_configured() {
        let mut policy = default_remote_media_policy();
        policy.block_loopback_ips = false;
        policy.blocked_hostnames.retain(|host| !host.eq_ignore_ascii_case("localhost"));

        let validated =
            validate_remote_media_url_with_policy("http://127.0.0.1/demo.png", &policy).unwrap();
        assert_eq!(validated, "http://127.0.0.1/demo.png");
    }

    #[test]
    fn validate_remote_media_url_with_policy_blocks_custom_host_suffix() {
        let mut policy = default_remote_media_policy();
        policy.blocked_host_suffixes.push(".corp".to_string());

        let err =
            validate_remote_media_url_with_policy("https://media.example.corp/demo.png", &policy)
                .unwrap_err();
        assert!(err.to_string().contains("blocked host"));
    }

    #[test]
    fn resolve_outbound_media_local_roots_returns_none_for_empty_input() {
        assert_eq!(resolve_outbound_media_local_roots(None), None);
        assert_eq!(resolve_outbound_media_local_roots(Some(Vec::new())), None);
    }

    #[test]
    fn build_outbound_media_load_options_keeps_explicit_local_roots() {
        let options = build_outbound_media_load_options(
            1024,
            Some(Path::new("/tmp/workspace")),
            Some(vec![PathBuf::from("/tmp/workspace-agent")]),
            LocalPathPolicy::DefaultRoots,
        );

        assert_eq!(options.max_bytes, 1024);
        assert_eq!(options.workspace_dir, Some(PathBuf::from("/tmp/workspace")));
        assert_eq!(
            options.local_roots,
            Some(vec![PathBuf::from("/tmp/workspace-agent")])
        );
        assert!(options.read_local_bytes.is_none());
    }

    #[test]
    fn build_sandbox_validated_media_load_options_sets_reader_and_policy() {
        let root = Path::new("/tmp/sandbox-root");
        let options = build_sandbox_validated_media_load_options(
            2048,
            root,
            Arc::new(|path| std::fs::read(path).map_err(Into::into)),
        );

        assert_eq!(options.max_bytes, 2048);
        assert_eq!(options.workspace_dir, Some(root.to_path_buf()));
        assert_eq!(options.local_path_policy, LocalPathPolicy::SandboxValidated);
        assert!(options.read_local_bytes.is_some());
    }

    #[test]
    fn build_root_scoped_sandbox_media_load_options_sets_sandbox_defaults() {
        let root = Path::new("/tmp/sandbox-root");
        let options = build_root_scoped_sandbox_media_load_options(2048, root);

        assert_eq!(options.max_bytes, 2048);
        assert_eq!(options.workspace_dir, Some(root.to_path_buf()));
        assert_eq!(options.local_path_policy, LocalPathPolicy::SandboxValidated);
        assert!(options.read_local_bytes.is_some());
    }

    #[test]
    fn build_root_scoped_local_reader_rejects_path_outside_root() {
        let root = TempDir::new().unwrap();
        let other = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let file = other.path().join("demo.png");
        std::fs::write(&file, b"png").unwrap();

        let reader = build_root_scoped_local_reader(root.path());
        let err = reader(&file).unwrap_err();

        assert!(err.to_string().contains("not under an allowed directory"));
    }

    #[test]
    fn build_root_scoped_local_reader_reads_path_within_root() {
        let root = TempDir::new().unwrap();
        let file = root.path().join("demo.png");
        std::fs::write(&file, b"png").unwrap();

        let reader = build_root_scoped_local_reader(root.path());
        let bytes = reader(&file).unwrap();

        assert_eq!(bytes, b"png");
    }

    #[tokio::test]
    async fn load_outbound_media_allows_explicit_local_path_policy() {
        let repo_root = std::env::current_dir().unwrap();
        let outside = tempfile::tempdir_in(repo_root).unwrap();
        let file = outside.path().join("demo.png");
        std::fs::write(&file, b"png").unwrap();

        let loaded = load_outbound_media(
            &reqwest::Client::new(),
            &file.display().to_string(),
            &build_outbound_media_load_options(
                1024,
                None,
                None,
                LocalPathPolicy::ExplicitPathAllowed,
            ),
        )
        .await
        .unwrap();

        assert!(matches!(loaded, LoadedOutboundMedia::LocalPath(_)));
    }

    #[tokio::test]
    async fn load_outbound_media_honors_explicit_local_roots() {
        let repo_root = std::env::current_dir().unwrap();
        let outside = tempfile::tempdir_in(repo_root).unwrap();
        let file = outside.path().join("demo.png");
        std::fs::write(&file, b"png").unwrap();

        let err = load_outbound_media(
            &reqwest::Client::new(),
            &file.display().to_string(),
            &build_outbound_media_load_options(1024, None, None, LocalPathPolicy::DefaultRoots),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not under an allowed directory"));

        let loaded = load_outbound_media(
            &reqwest::Client::new(),
            &file.display().to_string(),
            &build_outbound_media_load_options(
                1024,
                None,
                Some(vec![outside.path().to_path_buf()]),
                LocalPathPolicy::DefaultRoots,
            ),
        )
        .await
        .unwrap();

        assert!(matches!(loaded, LoadedOutboundMedia::LocalPath(_)));
    }

    #[tokio::test]
    async fn load_outbound_media_sandbox_validated_requires_explicit_reader_for_in_memory_bypass() {
        let repo_root = std::env::current_dir().unwrap();
        let outside = tempfile::tempdir_in(repo_root).unwrap();
        let file = outside.path().join("demo.png");
        std::fs::write(&file, b"png").unwrap();

        let mut options =
            build_outbound_media_load_options(1024, None, None, LocalPathPolicy::SandboxValidated);
        options.read_local_bytes = Some(Arc::new(|path| std::fs::read(path).map_err(Into::into)));

        let loaded = load_outbound_media(
            &reqwest::Client::new(),
            &file.display().to_string(),
            &options,
        )
        .await
        .unwrap();

        assert!(matches!(loaded, LoadedOutboundMedia::InMemory(_)));
    }

    #[tokio::test]
    async fn load_outbound_media_any_roots_with_reader_requires_reader() {
        let repo_root = std::env::current_dir().unwrap();
        let outside = tempfile::tempdir_in(repo_root).unwrap();
        let file = outside.path().join("demo.png");
        std::fs::write(&file, b"png").unwrap();

        let err = load_outbound_media(
            &reqwest::Client::new(),
            &file.display().to_string(),
            &build_outbound_media_load_options(1024, None, None, LocalPathPolicy::AnyRootsWithReader),
        )
        .await
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("Refusing local roots bypass without read_local_bytes override"));

        let mut options =
            build_outbound_media_load_options(1024, None, None, LocalPathPolicy::AnyRootsWithReader);
        options.read_local_bytes = Some(Arc::new(|path| std::fs::read(path).map_err(Into::into)));

        let loaded = load_outbound_media(
            &reqwest::Client::new(),
            &file.display().to_string(),
            &options,
        )
        .await
        .unwrap();

        assert!(matches!(loaded, LoadedOutboundMedia::InMemory(_)));
    }
}
