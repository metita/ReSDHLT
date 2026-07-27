//! Compiles the Windows resources (icon + version info) into the executable.
//!
//! This is done by calling the SDK's `rc.exe` directly and handing the result
//! to the linker, rather than pulling in a crate for it: the build stays
//! dependency-free, and a machine without the SDK just gets an icon-less
//! binary instead of a broken build.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/resdhlt.ico");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        // The GNU toolchain wants windres and a different link flag; not worth
        // guessing at, and the icon is cosmetic.
        return;
    }

    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe no encontrado: el .exe queda sin icono");
        return;
    };

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let res = out_dir.join("resdhlt.res");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    // The version block is generated rather than checked in: a hardcoded one
    // silently kept saying 0.1.0 in the file properties for every release after
    // the first.
    let script = out_dir.join("resdhlt.rc");
    if let Err(e) = std::fs::write(&script, version_rc(&manifest)) {
        println!("cargo:warning=no pude escribir el .rc ({e}): sin icono");
        return;
    }

    let status = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&script)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bins={}", res.display());
        }
        Ok(s) => println!("cargo:warning=rc.exe falló ({s}): el .exe queda sin icono"),
        Err(e) => println!("cargo:warning=no pude ejecutar rc.exe ({e}): sin icono"),
    }
}

/// The resource script: the icon, and a version block built from the crate
/// version so the file properties never disagree with what the updater
/// compares.
fn version_rc(manifest: &Path) -> String {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor, patch) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    // Backslashes are escapes inside an .rc string.
    let icon = manifest
        .join("assets")
        .join("resdhlt.ico")
        .display()
        .to_string()
        .replace('\\', "\\\\");

    format!(
        r#"// Generado por build.rs. No editar: se reescribe en cada compilación.
1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION     {major},{minor},{patch},0
PRODUCTVERSION  {major},{minor},{patch},0
FILEOS          0x4L
FILETYPE        0x1L
{{
    BLOCK "StringFileInfo"
    {{
        BLOCK "080904B0"
        {{
            VALUE "CompanyName",      "ReSDHLT"
            VALUE "FileDescription",  "ReSDHLT - compilador de mapas para CS 1.6"
            VALUE "FileVersion",      "{version}"
            VALUE "InternalName",     "resdhlt-gui"
            VALUE "LegalCopyright",   "GPL-2.0-or-later"
            VALUE "OriginalFilename", "resdhlt-gui.exe"
            VALUE "ProductName",      "ReSDHLT"
            VALUE "ProductVersion",   "{version}"
        }}
    }}
    BLOCK "VarFileInfo"
    {{
        VALUE "Translation", 0x809, 1200
    }}
}}
"#
    )
}

/// Newest `rc.exe` from the installed Windows SDKs, matching the host
/// architecture. `cc`-style discovery without the dependency.
fn find_rc() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    let arch = if cfg!(target_arch = "x86") { "x86" } else { "x64" };
    let mut roots: Vec<PathBuf> = Vec::new();
    for env in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Ok(pf) = std::env::var(env) {
            roots.push(Path::new(&pf).join("Windows Kits").join("10").join("bin"));
        }
    }

    let mut best: Option<PathBuf> = None;
    let mut best_name = String::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // SDK versions sort lexically the same way they sort by version.
            if !name.starts_with("10.") || name <= best_name {
                continue;
            }
            let candidate = entry.path().join(arch).join("rc.exe");
            if candidate.is_file() {
                best_name = name;
                best = Some(candidate);
            }
        }
    }
    best
}
