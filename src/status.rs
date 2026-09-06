use serde::Serialize;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::id::data_dir;

static CLIENTS: AtomicU32 = AtomicU32::new(0);

#[derive(Serialize)]
struct Snapshot {
    id: String,
    id_pretty: String,
    hostname: String,
    endpoints: Vec<String>,
    clients: u32,
    port: u16,
    fingerprint: String,
}

pub fn client_connected() {
    CLIENTS.fetch_add(1, Ordering::SeqCst);
}

pub fn client_disconnected() {
    CLIENTS
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
            Some(v.saturating_sub(1))
        })
        .ok();
}

pub fn write(
    id: &str,
    hostname: &str,
    endpoints: &[String],
    fingerprint: &str,
) {
    let snap = Snapshot {
        id: id.to_string(),
        id_pretty: crate::id::format_id(id),
        hostname: hostname.to_string(),
        endpoints: endpoints.to_vec(),
        clients: CLIENTS.load(Ordering::SeqCst),
        port: crate::proto::PORT,
        fingerprint: fingerprint.to_string(),
    };
    let path = data_dir().join("status.json");
    if let Ok(s) = serde_json::to_string_pretty(&snap) {
        let _ = std::fs::write(path, s);
    }
}
