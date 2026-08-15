//! Updates from GitHub Releases.
//!
//! Releases, not commits: the check asks for `/releases/latest`, so pushing to
//! master changes nothing here and the GUI only offers an update when a release
//! is actually published.
//!
//! No HTTP crate is pulled in for this. `curl.exe` ships with Windows 10 and
//! later, and the JSON is parsed with the `serde_json` that was already a
//! dependency, so the whole feature costs no new code to audit and no TLS stack
//! to keep current.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};
use std::thread;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Where releases are published. Anything downloaded is checked against this
/// host, so a redirect cannot point the updater somewhere else.
pub const REPO: &str = "metita/ReSDHLT";
const API_HOST: &str = "https://api.github.com";
const ALLOWED_DOWNLOAD_HOSTS: [&str; 2] = [
    "https://github.com/",
    "https://objects.githubusercontent.com/",
];
// Public half of the release key. The private 32-byte seed lives only in the
// GitHub Actions secret RESDHLT_ED25519_PRIVATE_KEY_B64.
const RELEASE_PUBLIC_KEY_B64: &str = "iKjm08UlKxquw5vOfFDSmkqXnclWy30HypTUrRYEDQ8=";

// ---------------------------------------------------------------- version

/// A release version, compared piece by piece so 0.10.0 beats 0.9.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// Accepts "v1.2.3", "1.2.3", "1.2" and "1", ignoring anything after a
    /// suffix like "-beta".
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim().trim_start_matches(['v', 'V']);
        let core = t.split(['-', '+']).next().unwrap_or(t);
        let mut parts = core.split('.').map(|p| p.trim().parse::<u32>());
        let major = parts.next()?.ok()?;
        let minor = parts.next().transpose().ok().flatten().unwrap_or(0);
        let patch = parts.next().transpose().ok().flatten().unwrap_or(0);
        Some(Version(major, minor, patch))
    }

    pub fn current() -> Self {
        Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_default()
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

// ---------------------------------------------------------------- release

#[derive(Debug, Clone)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub notes: String,
    pub asset_name: String,
    pub asset_url: String,
    pub asset_size: u64,
    pub checksum_url: Option<String>,
    pub signature_url: Option<String>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// What the checking thread reports back.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A newer release exists.
    Available(Release),
    /// Checked fine, nothing newer.
    UpToDate,
    Failed(String),
}

pub struct Check {
    pub rx: Receiver<Msg>,
}

pub struct Install {
    pub rx: Receiver<Result<(), String>>,
}

#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
fn hide_console(_cmd: &mut Command) {}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("curl");
    cmd.args([
        "--silent",
        "--show-error",
        "--location",
        "--max-time",
        "120",
        "--user-agent",
        concat!("resdhlt-gui/", env!("CARGO_PKG_VERSION")),
    ]);
    cmd.args(args);
    hide_console(&mut cmd);

    let out = cmd
        .output()
        .map_err(|e| format!("no pude ejecutar curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("curl terminó con {}", out.status)
        } else {
            err
        });
    }
    Ok(out.stdout)
}

/// Asks GitHub for the latest release, in the background.
pub fn check() -> Check {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let _ = tx.send(match fetch_latest() {
            Ok(Some(release)) => Msg::Available(release),
            Ok(None) => Msg::UpToDate,
            Err(e) => Msg::Failed(e),
        });
    });
    Check { rx }
}

fn fetch_latest() -> Result<Option<Release>, String> {
    match latest_release(REPO)? {
        Some(release) if release.version > Version::current() => Ok(Some(release)),
        _ => Ok(None),
    }
}

/// The newest published release of a repo, or `None` when there is none.
///
/// `--fail` is deliberately not used here: a repo with no releases yet answers
/// 404 with a JSON body, and that is "nothing to update", not a failure to show
/// the user. Real problems (rate limits, network) still come back as errors.
fn latest_release(repo: &str) -> Result<Option<Release>, String> {
    let url = format!("{API_HOST}/repos/{repo}/releases/latest");
    let body = curl(&["--header", "Accept: application/vnd.github+json", &url])?;
    parse_release(&body)
}

fn parse_release(body: &[u8]) -> Result<Option<Release>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("respuesta ilegible: {e}"))?;

    if value.get("tag_name").is_none() {
        let message = value
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("respuesta inesperada de GitHub");
        return if message.eq_ignore_ascii_case("Not Found") {
            Ok(None) // no releases published yet
        } else {
            Err(message.to_string())
        };
    }

    let api: ApiRelease =
        serde_json::from_value(value).map_err(|e| format!("respuesta ilegible: {e}"))?;

    if api.draft || api.prerelease {
        return Ok(None);
    }
    let version =
        Version::parse(&api.tag_name).ok_or_else(|| format!("tag raro: {}", api.tag_name))?;

    // Never let GitHub's asset order choose an ISA-specific package. The GUI
    // updates to the portable build for this exact OS/architecture; AVX2 is an
    // explicit manual download and a Windows ZIP is never offered on Linux.
    let Some(expected_name) = expected_asset_name() else {
        return Ok(None);
    };
    let asset = api
        .assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(expected_name))
        .ok_or_else(|| format!("la release no trae el paquete portable {expected_name}"))?;

    let checksum_name = format!("{expected_name}.sha256");
    let checksum_asset = api
        .assets
        .iter()
        .find(|candidate| candidate.name.eq_ignore_ascii_case(&checksum_name));
    let signature_name = format!("{expected_name}.sig");
    let signature_asset = api
        .assets
        .iter()
        .find(|candidate| candidate.name.eq_ignore_ascii_case(&signature_name));
    let Some(signature_asset) = signature_asset else {
        return Err(format!(
            "la release no trae la firma Ed25519 {signature_name}"
        ));
    };

    if !ALLOWED_DOWNLOAD_HOSTS
        .iter()
        .any(|h| asset.browser_download_url.starts_with(h))
    {
        return Err(format!(
            "la descarga no apunta a GitHub: {}",
            asset.browser_download_url
        ));
    }
    if let Some(checksum) = checksum_asset {
        if !ALLOWED_DOWNLOAD_HOSTS
            .iter()
            .any(|host| checksum.browser_download_url.starts_with(host))
        {
            return Err(format!(
                "el checksum no apunta a GitHub: {}",
                checksum.browser_download_url
            ));
        }
    }
    if !ALLOWED_DOWNLOAD_HOSTS
        .iter()
        .any(|host| signature_asset.browser_download_url.starts_with(host))
    {
        return Err(format!(
            "la firma no apunta a GitHub: {}",
            signature_asset.browser_download_url
        ));
    }

    Ok(Some(Release {
        version,
        tag: api.tag_name.clone(),
        notes: api.body.unwrap_or_default(),
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
        asset_size: asset.size,
        checksum_url: checksum_asset.map(|checksum| checksum.browser_download_url.clone()),
        signature_url: Some(signature_asset.browser_download_url.clone()),
    }))
}

fn parse_sha256_manifest(text: &[u8], asset_name: &str) -> Result<String, String> {
    let text = std::str::from_utf8(text).map_err(|_| "checksum ilegible".to_string())?;
    let mut matching = None;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let mut fields = line.split_whitespace();
        let Some(hash) = fields.next() else { continue };
        let Some(name) = fields.next() else { continue };
        if fields.next().is_some() || name.trim_start_matches('*') != asset_name {
            continue;
        }
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("checksum SHA-256 inválido".to_string());
        }
        if matching.replace(hash.to_ascii_lowercase()).is_some() {
            return Err("el manifest repite el checksum del paquete".to_string());
        }
    }
    matching.ok_or_else(|| format!("el manifest no contiene {asset_name}"))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn verify_signature_with_key(
    path: &Path,
    encoded_signature: &[u8],
    key: &VerifyingKey,
) -> Result<(), String> {
    let signature_bytes = STANDARD
        .decode(
            std::str::from_utf8(encoded_signature)
                .map_err(|_| "firma ilegible".to_string())?
                .trim(),
        )
        .map_err(|_| "firma Ed25519 no es base64 válido".to_string())?;
    let signature_bytes: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| "la firma Ed25519 no tiene 64 bytes".to_string())?;
    let signature = Signature::from_bytes(&signature_bytes);
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    key.verify(&bytes, &signature)
        .map_err(|_| "la firma Ed25519 del paquete no coincide".to_string())
}

fn verify_release_signature(path: &Path, encoded_signature: &[u8]) -> Result<(), String> {
    let key_bytes = STANDARD
        .decode(RELEASE_PUBLIC_KEY_B64)
        .map_err(|_| "clave pública Ed25519 inválida".to_string())?;
    let key_bytes: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| "la clave pública Ed25519 no tiene 32 bytes".to_string())?;
    let key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|_| "clave pública Ed25519 inválida".to_string())?;
    verify_signature_with_key(path, encoded_signature, &key)
}

fn expected_asset_name() -> Option<&'static str> {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        Some("ReSDHLT-Windows-x64.zip")
    }
    #[cfg(all(windows, target_arch = "x86"))]
    {
        Some("ReSDHLT-Windows-x86.zip")
    }
    #[cfg(not(all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
    {
        None
    }
}

// ---------------------------------------------------------------- install

/// Downloads the release and hands the swap to a helper script.
///
/// A running .exe cannot overwrite itself, so the sequence is: download, unpack,
/// write a small script, start it, and quit. The script waits for this process
/// to be gone, copies the new files over the install folder and starts the GUI
/// again. If anything fails it leaves a log next to the executable rather than
/// a half-updated install.
pub fn install(release: &Release) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let install_dir = exe
        .parent()
        .ok_or("no pude determinar la carpeta de instalación")?
        .to_path_buf();

    let work = std::env::temp_dir().join(format!("resdhlt-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;

    let checksum_url = release.checksum_url.as_deref().ok_or_else(|| {
        format!(
            "la release {} no publica {}.sha256; actualización rechazada",
            release.tag, release.asset_name
        )
    })?;
    let checksum_body = curl(&["--fail", checksum_url])?;
    let expected_sha256 = parse_sha256_manifest(&checksum_body, &release.asset_name)?;

    let zip = work.join("package.zip");
    curl(&[
        "--fail",
        "--output",
        &zip.display().to_string(),
        &release.asset_url,
    ])?;

    let downloaded = std::fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
    if downloaded == 0 {
        return Err("la descarga quedó vacía".to_string());
    }
    if release.asset_size > 0 && downloaded != release.asset_size {
        return Err(format!(
            "la descarga no coincide con la release: {downloaded} bytes contra {}",
            release.asset_size
        ));
    }
    let actual_sha256 = sha256_file(&zip)?;
    if actual_sha256 != expected_sha256 {
        return Err(format!(
            "SHA-256 incorrecto para {}: esperado {}, recibido {}",
            release.asset_name, expected_sha256, actual_sha256
        ));
    }
    let signature_url = release.signature_url.as_deref().ok_or_else(|| {
        format!(
            "la release {} no publica {}.sig; actualización rechazada",
            release.tag, release.asset_name
        )
    })?;
    let signature_body = curl(&["--fail", signature_url])?;
    verify_release_signature(&zip, &signature_body)?;

    let unpacked = work.join("pkg");
    unzip(&zip, &unpacked)?;
    for required in [
        "resdhlt-gui.exe",
        "tools/sdHLCSG.exe",
        "tools/sdHLBSP.exe",
        "tools/sdHLVIS.exe",
        "tools/sdHLRAD.exe",
        "tools/sdRIPENT.exe",
        "tools/settings.txt",
        "tools/sdhlt.wad",
    ] {
        if !unpacked.join(required).is_file() {
            return Err(format!("el paquete está incompleto: falta {required}"));
        }
    }

    spawn_swapper(&unpacked, &install_dir, &exe, &work)
}

/// Downloading and unpacking can take minutes on a slow link. Keep that work
/// off the egui thread so progress, cancel buttons and window repainting stay
/// responsive.
pub fn install_async(release: Release) -> Install {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let _ = tx.send(install(&release));
    });
    Install { rx }
}

#[cfg(windows)]
fn unzip(zip: &Path, dest: &Path) -> Result<(), String> {
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            ps_literal(zip),
            ps_literal(dest)
        ),
    ]);
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "no pude descomprimir: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn ps_literal(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}

#[cfg(not(windows))]
fn unzip(zip: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let out = Command::new("unzip")
        .args([
            "-o",
            &zip.display().to_string(),
            "-d",
            &dest.display().to_string(),
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("no pude descomprimir".to_string());
    }
    Ok(())
}

#[cfg(windows)]
fn spawn_swapper(src: &Path, dest: &Path, exe: &Path, work: &Path) -> Result<(), String> {
    let log = dest.join("resdhlt-update.log");
    let script = work.join("apply.ps1");
    let body = format!(
        r#"$ErrorActionPreference = 'Stop'
Start-Transcript -Path '{log}' -Force | Out-Null
$backup = Join-Path '{work}' 'backup'
$installed = @()
try {{
    # The GUI is still shutting down; its files stay locked until it is gone.
    Wait-Process -Id {pid} -Timeout 60 -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 400
    if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{
        throw 'la GUI no terminó dentro de 60 segundos'
    }}

    New-Item -ItemType Directory -Path $backup -Force | Out-Null
    $incoming = Get-ChildItem -LiteralPath '{src}' -Force
    foreach ($item in $incoming) {{
        $target = Join-Path '{dest}' $item.Name
        if (Test-Path -LiteralPath $target) {{
            Move-Item -LiteralPath $target -Destination $backup -Force
        }}
        $installed += $target
        Copy-Item -LiteralPath $item.FullName -Destination $target -Recurse -Force
    }}
    Start-Process -FilePath '{exe}'
}} catch {{
    Write-Output "FALLO: $_"
    foreach ($target in $installed) {{
        if (Test-Path -LiteralPath $target) {{
            Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction SilentlyContinue
        }}
    }}
    if (Test-Path -LiteralPath $backup) {{
        Get-ChildItem -LiteralPath $backup -Force | ForEach-Object {{
            Move-Item -LiteralPath $_.FullName -Destination '{dest}' -Force
        }}
    }}
    Write-Output 'ROLLBACK: instalación anterior restaurada'
}} finally {{
    Stop-Transcript | Out-Null
}}
"#,
        log = ps_literal(&log),
        pid = std::process::id(),
        src = ps_literal(src),
        dest = ps_literal(dest),
        exe = ps_literal(exe),
        work = ps_literal(work),
    );
    std::fs::write(&script, body).map_err(|e| e.to_string())?;

    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        &script.display().to_string(),
    ]);
    hide_console(&mut cmd);
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_swapper(_src: &Path, _dest: &Path, _exe: &Path, _work: &Path) -> Result<(), String> {
    Err("el actualizador automático todavía no está disponible en esta plataforma".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("0.1"), Some(Version(0, 1, 0)));
        assert_eq!(Version::parse("2"), Some(Version(2, 0, 0)));
        assert_eq!(Version::parse("v0.3.0-beta1"), Some(Version(0, 3, 0)));
        assert_eq!(Version::parse("no soy una versión"), None);

        // The reason this is parsed instead of compared as strings.
        assert!(Version::parse("v0.10.0") > Version::parse("v0.9.0"));
        assert!(Version::parse("v1.0.0") > Version::parse("v0.99.99"));
        assert!(Version::parse("v0.1.0") == Version::parse("0.1.0"));
    }

    /// A repo with no releases yet - which is what this one looks like until
    /// the first tag is pushed - must read as "nothing new", not as an error.
    #[test]
    fn a_repo_without_releases_is_not_an_error() {
        let body = br#"{"message":"Not Found","status":"404"}"#;
        assert!(matches!(parse_release(body), Ok(None)));
    }

    /// Anything else GitHub says is a real problem and has to reach the user.
    #[test]
    fn other_api_problems_are_reported() {
        let body = br#"{"message":"API rate limit exceeded"}"#;
        match parse_release(body) {
            Err(e) => assert!(e.contains("rate limit")),
            other => panic!("esperaba error, salió {other:?}"),
        }
    }

    #[test]
    #[cfg(all(windows, target_arch = "x86_64"))]
    fn picks_the_portable_windows_zip_even_when_avx2_comes_first() {
        let body = br#"{
            "tag_name": "v9.9.9",
            "body": "notas",
            "assets": [
                {"name":"ReSDHLT-Windows-x64-avx2.zip","browser_download_url":"https://github.com/x/y/avx2.zip","size":99},
                {"name":"source.tar.gz","browser_download_url":"https://github.com/x/y/a.tar.gz","size":1},
                {"name":"ReSDHLT-Windows-x64.zip","browser_download_url":"https://github.com/x/y/w.zip","size":42},
                {"name":"ReSDHLT-Windows-x64.zip.sha256","browser_download_url":"https://github.com/x/y/w.zip.sha256","size":96},
                {"name":"ReSDHLT-Windows-x64.zip.sig","browser_download_url":"https://github.com/x/y/w.zip.sig","size":88}
            ]
        }"#;
        let r = parse_release(body).unwrap().unwrap();
        assert_eq!(r.version, Version(9, 9, 9));
        assert_eq!(r.asset_name, "ReSDHLT-Windows-x64.zip");
        assert_eq!(r.asset_url, "https://github.com/x/y/w.zip");
        assert_eq!(r.asset_size, 42);
        assert_eq!(
            r.checksum_url.as_deref(),
            Some("https://github.com/x/y/w.zip.sha256")
        );
        assert_eq!(
            r.signature_url.as_deref(),
            Some("https://github.com/x/y/w.zip.sig")
        );
    }

    #[test]
    fn sha256_manifest_requires_one_valid_entry_for_the_asset() {
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let manifest = format!("{hash} *ReSDHLT-Windows-x64.zip\n");
        assert_eq!(
            parse_sha256_manifest(manifest.as_bytes(), "ReSDHLT-Windows-x64.zip").unwrap(),
            hash
        );
        assert!(parse_sha256_manifest(
            b"not-a-hash *ReSDHLT-Windows-x64.zip\n",
            "ReSDHLT-Windows-x64.zip"
        )
        .is_err());
        assert!(
            parse_sha256_manifest(manifest.as_bytes(), "ReSDHLT-Windows-x64-avx2.zip").is_err()
        );
    }

    #[test]
    fn sha256_file_matches_a_known_vector() {
        let path =
            std::env::temp_dir().join(format!("resdhlt-sha256-test-{}.bin", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ed25519_signature_accepts_the_release_key_and_rejects_tampering() {
        let seed = STANDARD
            .decode("3a7gdHrMRAiav1nB+jMHpTph4KrNsJ4r7e0NoIAKRJM=")
            .unwrap();
        let key = ed25519_dalek::SigningKey::from_bytes(&seed.try_into().unwrap());
        let path =
            std::env::temp_dir().join(format!("resdhlt-signature-test-{}.bin", std::process::id()));
        std::fs::write(&path, b"signed package").unwrap();
        let signature = ed25519_dalek::Signer::sign(&key, b"signed package");
        let encoded = STANDARD.encode(signature.to_bytes());
        assert!(verify_signature_with_key(&path, encoded.as_bytes(), &key.verifying_key()).is_ok());
        std::fs::write(&path, b"tampered package").unwrap();
        assert!(
            verify_signature_with_key(&path, encoded.as_bytes(), &key.verifying_key()).is_err()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[cfg(not(windows))]
    fn non_windows_never_falls_back_to_the_windows_zip() {
        let body = br#"{
            "tag_name": "v9.9.9",
            "assets": [
                {"name":"ReSDHLT-Windows-x64.zip","browser_download_url":"https://github.com/x/y/w.zip","size":42}
            ]
        }"#;
        assert!(matches!(parse_release(body), Ok(None)));
    }

    #[test]
    fn drafts_and_prereleases_are_ignored() {
        let draft = br#"{"tag_name":"v9.9.9","draft":true,"assets":[]}"#;
        assert!(matches!(parse_release(draft), Ok(None)));
        let pre = br#"{"tag_name":"v9.9.9","prerelease":true,"assets":[]}"#;
        assert!(matches!(parse_release(pre), Ok(None)));
    }

    /// The real thing: ask this repo for its latest release, download the
    /// asset the way the updater does, and check the package has what a user
    /// needs. Skipped when there is no network or no release yet.
    #[test]
    fn downloads_and_unpacks_the_published_release() {
        let release = match latest_release(REPO) {
            Ok(Some(r)) => r,
            Ok(None) => return, // nothing published yet
            Err(e) => {
                eprintln!("sin red o API limitada, salteado: {e}");
                return;
            }
        };

        let work = std::env::temp_dir().join("resdhlt-update-test");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let zip = work.join("package.zip");

        if let Err(e) = curl(&[
            "--fail",
            "--output",
            &zip.display().to_string(),
            &release.asset_url,
        ]) {
            eprintln!("descarga no disponible, salteado: {e}");
            return;
        }

        let size = std::fs::metadata(&zip).unwrap().len();
        assert_eq!(
            size, release.asset_size,
            "el .zip no coincide con la release"
        );

        let out = work.join("pkg");
        unzip(&zip, &out).unwrap();
        for needed in [
            "resdhlt-gui.exe",
            "tools/sdHLCSG.exe",
            "tools/sdHLBSP.exe",
            "tools/sdHLVIS.exe",
            "tools/sdHLRAD.exe",
            "tools/sdhlt.wad",
        ] {
            assert!(out.join(needed).is_file(), "falta {needed} en la release");
        }
        let _ = std::fs::remove_dir_all(&work);
    }

    /// End to end against GitHub, using a repo that does publish releases.
    /// Skipped rather than failed when the network is not available, so the
    /// suite still runs offline.
    #[test]
    fn reads_a_real_release_from_github() {
        match latest_release("seedee/SDHLT") {
            Ok(Some(r)) => {
                assert!(r.version >= Version(1, 0, 0), "{:?}", r.version);
                assert!(
                    r.asset_name.to_lowercase().ends_with(".zip"),
                    "{}",
                    r.asset_name
                );
                assert!(
                    r.asset_url.starts_with("https://github.com/"),
                    "{}",
                    r.asset_url
                );
            }
            Ok(None) => {}
            Err(e) => eprintln!("sin red o API limitada, salteado: {e}"),
        }
    }

    #[test]
    fn downloads_must_come_from_github() {
        let evil = "https://evil.example.com/ReSDHLT-Windows-x64.zip";
        assert!(!ALLOWED_DOWNLOAD_HOSTS.iter().any(|h| evil.starts_with(h)));
        let good =
            "https://github.com/metita/ReSDHLT/releases/download/v0.2.0/ReSDHLT-Windows-x64.zip";
        assert!(ALLOWED_DOWNLOAD_HOSTS.iter().any(|h| good.starts_with(h)));
    }
}
