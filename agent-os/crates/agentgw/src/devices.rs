//! Per-device auth registry (Phase 14 slice): the gateway authenticates
//! remote/secondary callers by a per-device bearer token stored as a
//! SHA-256 hash — the file never holds plaintext tokens, and `revoke`
//! flips `enabled` without rewriting anyone else's credentials.
//!
//! The registry file is re-read on each authenticated request, so
//! `device revoke` takes effect without a gateway restart.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One registered device.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DeviceEntry {
    /// `sha256:` hex of the bearer token — never the token itself.
    pub token_hash: String,
    /// Human label ("laptop", "ci-bot").
    pub label: String,
    /// Revoked devices stay listed for audit but cannot authenticate.
    pub enabled: bool,
    /// Registration time (unix ms).
    pub created_at_ms: i64,
}

/// The on-disk registry document.
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DeviceRegistry {
    /// device id → entry.
    pub devices: BTreeMap<String, DeviceEntry>,
}

fn io_err(msg: &str, e: io::Error) -> io::Error {
    io::Error::other(format!("{msg}: {e}"))
}

/// Loads the registry; a missing file yields an empty registry.
pub fn load(path: &Path) -> io::Result<DeviceRegistry> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| io::Error::other(format!("devices file not valid JSON: {e}"))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(DeviceRegistry::default()),
        Err(e) => Err(io_err("read devices file", e)),
    }
}

fn save(path: &Path, reg: &DeviceRegistry) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(reg)
        .map_err(|e| io::Error::other(format!("encode devices file: {e}")))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| io_err("write devices file", e))?;
    std::fs::rename(&tmp, path).map_err(|e| io_err("rename devices file", e))
}

/// Registers a device and returns `(device_id, token)` — the token is
/// shown once at creation and only its hash is persisted.
pub fn add(path: &Path, label: &str) -> io::Result<(String, String)> {
    let mut reg = load(path)?;
    let device_id = uuid::Uuid::now_v7().to_string();
    // Two v7 UUIDs give ~148 random bits — ample for a bearer token.
    let token = format!(
        "gwdev_{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default();
    reg.devices.insert(
        device_id.clone(),
        DeviceEntry {
            token_hash: token_hash(&token),
            label: label.to_owned(),
            enabled: true,
            created_at_ms: now,
        },
    );
    save(path, &reg)?;
    Ok((device_id, token))
}

/// Disables a device by id (prefix-resolved like adapter ids).
pub fn revoke(path: &Path, id_prefix: &str) -> io::Result<String> {
    let mut reg = load(path)?;
    let matches: Vec<String> = reg
        .devices
        .keys()
        .filter(|k| k.starts_with(id_prefix))
        .cloned()
        .collect();
    let id = match matches.as_slice() {
        [id] => id.clone(),
        [] => {
            return Err(io::Error::other(format!(
                "no device matching '{id_prefix}'"
            )));
        }
        _ => {
            return Err(io::Error::other(format!(
                "device prefix '{id_prefix}' is ambiguous"
            )));
        }
    };
    if let Some(entry) = reg.devices.get_mut(&id) {
        entry.enabled = false;
    }
    save(path, &reg)?;
    Ok(id)
}

/// Hashes a bearer token for storage/lookup.
fn token_hash(token: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(token.as_bytes());
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// Authenticates a bearer token against the current file contents —
/// returns the enabled device id on match.
pub fn authenticate(path: &Path, token: &str) -> Option<String> {
    let reg = load(path).ok()?;
    let hash = token_hash(token);
    reg.devices
        .iter()
        .find_map(|(id, e)| (e.enabled && e.token_hash == hash).then(|| id.clone()))
}

/// `agentgw device <add|revoke|list>` — standalone CLI entry.
pub fn cli(args: &[String]) -> i32 {
    let mut file: Option<PathBuf> = None;
    let mut label = String::from("device");
    let mut pos: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--devices-file" if i + 1 < args.len() => {
                file = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            a if a.starts_with("--devices-file=") => {
                file = Some(PathBuf::from(&a["--devices-file=".len()..]));
                i += 1;
            }
            "--label" if i + 1 < args.len() => {
                label = args[i + 1].clone();
                i += 2;
            }
            a if a.starts_with("--label=") => {
                label = a["--label=".len()..].to_owned();
                i += 1;
            }
            a => {
                pos.push(a.to_owned());
                i += 1;
            }
        }
    }
    let Some(file) = file else {
        eprintln!("device commands require --devices-file <path>");
        return 2;
    };
    match pos.first().map(String::as_str) {
        Some("add") => match add(&file, &label) {
            Ok((id, token)) => {
                println!(
                    "{}",
                    serde_json::json!({"device_id": id, "label": label, "token": token})
                );
                eprintln!("store the token now — only its hash is kept on disk");
                0
            }
            Err(e) => {
                eprintln!("device add failed: {e}");
                1
            }
        },
        Some("revoke") => match pos.get(1).map(String::as_str) {
            Some(id) => match revoke(&file, id) {
                Ok(id) => {
                    println!("{}", serde_json::json!({"revoked": id}));
                    0
                }
                Err(e) => {
                    eprintln!("device revoke failed: {e}");
                    1
                }
            },
            None => {
                eprintln!("device revoke <id-prefix> required");
                2
            }
        },
        Some("list") => match load(&file) {
            Ok(reg) => {
                let rows: Vec<_> = reg
                    .devices
                    .iter()
                    .map(|(id, e)| {
                        serde_json::json!({
                            "device_id": id,
                            "label": e.label,
                            "enabled": e.enabled,
                            "created_at_ms": e.created_at_ms,
                        })
                    })
                    .collect();
                println!("{}", serde_json::json!({"devices": rows}));
                0
            }
            Err(e) => {
                eprintln!("device list failed: {e}");
                1
            }
        },
        _ => {
            eprintln!(
                "usage: agentgw device <add|revoke|list> --devices-file <path> [--label L] [id]"
            );
            2
        }
    }
}
