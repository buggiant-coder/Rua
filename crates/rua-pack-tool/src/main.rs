mod keychain;
mod sign;

use sign::{
    decode_pubkey, encode_privkey, encode_pubkey, generate_keypair, load_signing_key_raw,
    sign_bytes, verify_bytes,
};
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        process::exit(1);
    }
    if let Err(e) = dispatch(&args[1], &args[2..]) {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn usage() {
    eprintln!("Usage: rua-pack-tool <command> [options]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  gen-keypair --out <name>           Generate Ed25519 keypair");
    eprintln!("  sign-zip   --key <id>|--key-file <path> --in <file> --out <sig>");
    eprintln!("  sign-file  --key <id>|--key-file <path> --in <file> --out <sig>");
    eprintln!("  verify     --pubkey <b64> --in <file> --sig <path>");
}

fn dispatch(cmd: &str, args: &[String]) -> Result<(), String> {
    match cmd {
        "gen-keypair" => cmd_gen_keypair(args),
        "sign-zip" | "sign-file" => cmd_sign(args),
        "verify" => cmd_verify(args),
        _ => {
            usage();
            Err(format!("unknown command: {cmd}"))
        }
    }
}

fn cmd_gen_keypair(args: &[String]) -> Result<(), String> {
    let out = require_flag(args, "--out")?;
    let (sk, vk) = generate_keypair();

    let pub_path = format!("{out}.pub");
    let priv_path = format!("{out}.priv");

    fs::write(&pub_path, encode_pubkey(&vk)).map_err(|e| format!("write {pub_path}: {e}"))?;
    fs::write(&priv_path, encode_privkey(&sk)).map_err(|e| format!("write {priv_path}: {e}"))?;

    println!("public key:  {}", encode_pubkey(&vk));
    println!("wrote {pub_path}");
    println!("wrote {priv_path}");
    eprintln!("WARNING: encrypt {priv_path} with GPG before storing to backup media");
    Ok(())
}

fn cmd_sign(args: &[String]) -> Result<(), String> {
    let input = require_flag(args, "--in")?;
    let out = require_flag(args, "--out")?;
    let sk = resolve_signing_key(args)?;

    let data = fs::read(&input).map_err(|e| format!("read {input}: {e}"))?;
    let sig = sign_bytes(&sk, &data);
    fs::write(&out, sig.to_bytes()).map_err(|e| format!("write {out}: {e}"))?;
    println!("signed {input} -> {out}");
    Ok(())
}

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let pubkey_b64 = require_flag(args, "--pubkey")?;
    let input = require_flag(args, "--in")?;
    let sig_path = require_flag(args, "--sig")?;

    let vk = decode_pubkey(&pubkey_b64)?;
    let data = fs::read(&input).map_err(|e| format!("read {input}: {e}"))?;
    let sig_bytes = fs::read(&sig_path).map_err(|e| format!("read {sig_path}: {e}"))?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "signature must be 64 bytes".to_string())?;
    let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);

    if verify_bytes(&vk, &data, &sig) {
        println!("OK: signature valid");
        Ok(())
    } else {
        Err("FAIL: signature invalid".into())
    }
}

fn resolve_signing_key(args: &[String]) -> Result<ed25519_dalek::SigningKey, String> {
    if let Ok(key_file) = require_flag(args, "--key-file") {
        let b64 = fs::read_to_string(&key_file).map_err(|e| format!("read {key_file}: {e}"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("bad base64 in {key_file}: {e}"))?;
        return load_signing_key_raw(&bytes);
    }
    if let Ok(key_id) = require_flag(args, "--key") {
        let bytes = keychain::read_privkey_from_keychain(&key_id)?;
        return load_signing_key_raw(&bytes);
    }
    Err("must provide --key <keychain-id> or --key-file <path>".into())
}

use base64::Engine;

fn require_flag(args: &[String], flag: &str) -> Result<String, String> {
    for (i, a) in args.iter().enumerate() {
        if a == flag {
            return args
                .get(i + 1)
                .map(|s| s.to_string())
                .ok_or_else(|| format!("{flag} requires a value"));
        }
    }
    Err(format!("missing {flag}"))
}
