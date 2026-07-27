//! Compiles the Windows resources (icon + version info) into the executable.
//!
//! This is done by calling the SDK's `rc.exe` directly and handing the result
//! to the linker, rather than pulling in a crate for it: the build stays
//! dependency-free, and a machine without the SDK just gets an icon-less
//! binary instead of a broken build.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.rc");
    println!("cargo:rerun-if-changed=assets/resdhlt.ico");

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

    let status = Command::new(&rc)
        .arg("/nologo")
        // The .rc references the .ico by bare name, so compile from the folder
        // that holds both.
        .arg("/fo")
        .arg(&res)
        .arg("icon.rc")
        .current_dir(manifest.join("assets"))
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bins={}", res.display());
        }
        Ok(s) => println!("cargo:warning=rc.exe falló ({s}): el .exe queda sin icono"),
        Err(e) => println!("cargo:warning=no pude ejecutar rc.exe ({e}): sin icono"),
    }
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
