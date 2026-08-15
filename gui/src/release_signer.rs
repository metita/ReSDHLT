//! Small CI-only Ed25519 signer for release ZIPs.
//!
//! The private seed is supplied through an environment variable, never stored
//! in the repository. The output is a plain base64 detached signature that the
//! GUI verifies before it starts the updater.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signer, SigningKey};
use rand_core::OsRng;
use std::env;
use std::fs;
use std::path::PathBuf;

const EXPECTED_PUBLIC_KEY_B64: &str = "l4LNeBTN4mH+ibxCfU2M0Hg/AYmRiy2Qwpn2jaIo3uM=";

fn usage() -> ! {
    eprintln!("uso: resdhlt-release-signer <zip> <firma.sig>");
    std::process::exit(2);
}

fn main() {
    let mut args = env::args_os();
    let _ = args.next();
    if args.len() == 1 && args.next().as_deref() == Some(std::ffi::OsStr::new("--generate-key")) {
        let key = SigningKey::generate(&mut OsRng);
        println!("private_seed_b64={}", STANDARD.encode(key.to_bytes()));
        println!(
            "public_key_b64={}",
            STANDARD.encode(key.verifying_key().to_bytes())
        );
        return;
    }
    let Some(input) = args.next().map(PathBuf::from) else {
        usage()
    };
    let Some(output) = args.next().map(PathBuf::from) else {
        usage()
    };
    if args.next().is_some() {
        usage();
    }

    let encoded = env::var("RESDHLT_ED25519_PRIVATE_KEY_B64")
        .unwrap_or_else(|_| panic!("falta RESDHLT_ED25519_PRIVATE_KEY_B64"));
    let seed = STANDARD
        .decode(encoded.trim())
        .unwrap_or_else(|_| panic!("RESDHLT_ED25519_PRIVATE_KEY_B64 no es base64"));
    let seed: [u8; 32] = seed
        .try_into()
        .unwrap_or_else(|_| panic!("la clave privada debe ser una semilla Ed25519 de 32 bytes"));
    let key = SigningKey::from_bytes(&seed);
    if STANDARD.encode(key.verifying_key().to_bytes()) != EXPECTED_PUBLIC_KEY_B64 {
        panic!("la clave privada no corresponde a la clave pública de releases");
    }
    let bytes = fs::read(&input).unwrap_or_else(|e| panic!("{}: {e}", input.display()));
    let signature = key.sign(&bytes);
    fs::write(&output, STANDARD.encode(signature.to_bytes()) + "\n")
        .unwrap_or_else(|e| panic!("{}: {e}", output.display()));
}
