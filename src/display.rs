use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::sync::Mutex;

use crate::id::data_dir;
use crate::proto::Viewport;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedOutput {
    pub name: String,
    pub mode: String,
    pub scale: String,
    pub rotation: String,
}

static SAVED: Mutex<Option<SavedOutput>> = Mutex::new(None);

pub fn session_env() -> Vec<(String, String)> {
    let uid = unsafe { libc::getuid() };
    let runtime = format!("/run/user/{uid}");
    vec![
        ("XDG_RUNTIME_DIR".into(), runtime.clone()),
        (
            "DBUS_SESSION_BUS_ADDRESS".into(),
            format!("unix:path={runtime}/bus"),
        ),
        ("XDG_CURRENT_DESKTOP".into(), "KDE".into()),
        ("XDG_SESSION_TYPE".into(), "wayland".into()),
    ]
}

pub fn session_live() -> bool {
    let uid = unsafe { libc::getuid() };
    let runtime = format!("/run/user/{uid}");
    std::path::Path::new(&format!("{runtime}/bus")).exists()
        && (std::path::Path::new(&format!("{runtime}/wayland-0")).exists()
            || std::path::Path::new(&format!("{runtime}/wayland-1")).exists())
}

/// Logical size of the primary output (the coordinate space KWin uses for the pointer).
pub fn logical_size() -> (u32, u32) {
    if let Ok(out) = kscreen(&["-o"]) {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("Geometry:") {
                // Geometry: 0,0 1920x1200
                if let Some(wh) = rest.split_whitespace().nth(1) {
                    if let Some((w, h)) = wh.split_once('x') {
                        if let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) {
                            return (w.max(1), h.max(1));
                        }
                    }
                }
            }
        }
    }
    (1920, 1200)
}

fn kscreen(args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = Command::new("kscreen-doctor");
    for (k, v) in session_env() {
        cmd.env(k, v);
    }
    cmd.args(args);
    Ok(cmd.output()?)
}

pub fn parse_primary(output: &str) -> Option<SavedOutput> {
    let mut name = None;
    let mut mode = None;
    let mut scale = None;
    let mut rotation = None;
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Output:") {
            // Output: 1 eDP-1 ...
            let parts: Vec<_> = t.split_whitespace().collect();
            if parts.len() >= 3 {
                name = Some(parts[2].to_string());
            }
        }
        if t.starts_with("Modes:") {
            for tok in t.split_whitespace() {
                if let Some(stripped) = tok.strip_suffix("!*") {
                    mode = Some(stripped.split(':').nth(1).unwrap_or(stripped).to_string());
                } else if let Some(stripped) = tok.strip_suffix('*') {
                    if stripped.contains('x') {
                        mode = Some(
                            stripped
                                .split(':')
                                .nth(1)
                                .unwrap_or(stripped)
                                .trim_end_matches('!')
                                .to_string(),
                        );
                    }
                } else if tok.contains("!*") || tok.ends_with('*') || tok.contains("*!") {
                    let s = tok.replace("!*", "").replace('*', "").replace('!', "");
                    if let Some((_, m)) = s.split_once(':') {
                        mode = Some(m.to_string());
                    }
                }
            }
        }
        if t.starts_with("Scale:") {
            scale = t.split_whitespace().nth(1).map(|s| s.to_string());
        }
        if t.starts_with("Rotation:") {
            let r = t.split_whitespace().nth(1).unwrap_or("1");
            rotation = Some(
                match r {
                    "1" => "none",
                    "2" => "right",
                    "4" => "inverted",
                    "8" => "left",
                    _ => "none",
                }
                .to_string(),
            );
        }
    }
    Some(SavedOutput {
        name: name?,
        mode: mode.unwrap_or_default(),
        scale: scale.unwrap_or_else(|| "1".into()),
        rotation: rotation.unwrap_or_else(|| "none".into()),
    })
}

pub fn save_current() -> Result<Option<SavedOutput>> {
    if !session_live() {
        return Ok(None);
    }
    let out = kscreen(&["-o"])?;
    let text = String::from_utf8_lossy(&out.stdout);
    let saved = parse_primary(&text);
    if let Some(ref s) = saved {
        let path = data_dir().join("saved-output.json");
        let _ = std::fs::write(path, serde_json::to_string_pretty(s)?);
        *SAVED.lock().unwrap() = Some(s.clone());
        tracing::info!(
            "saved display {} mode={} scale={} rot={}",
            s.name,
            s.mode,
            s.scale,
            s.rotation
        );
    }
    Ok(saved)
}

pub fn restore() {
    let saved = SAVED.lock().unwrap().clone().or_else(|| {
        let path = data_dir().join("saved-output.json");
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    });
    let Some(s) = saved else {
        return;
    };
    if !session_live() {
        return;
    }
    let spec_mode = format!("output.{}.mode.{}", s.name, s.mode);
    let spec_scale = format!("output.{}.scale.{}", s.name, s.scale);
    let spec_rot = format!("output.{}.rotation.{}", s.name, s.rotation);
    match kscreen(&[&spec_mode, &spec_scale, &spec_rot]) {
        Ok(o) if o.status.success() => tracing::info!("restored display {}", s.name),
        Ok(o) => tracing::warn!(
            "restore display failed: {}",
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => tracing::warn!("restore display: {e}"),
    }
}

/// Encode the physical desktop at a size that does **not** depend on phone
/// orientation. Rotating the client used to change 1600x738 ↔ 1600x1000,
/// which restarted gpu-screen-recorder and froze the picture.
pub fn encode_size(_vp: &Viewport) -> (u32, u32) {
    let (sw, sh) = logical_size();
    fit_even(sw.max(640), sh.max(400), 1600, 1000)
}

/// Pick a host mode + scale so logical size stays close to the client's
/// point size (readable without pinch-zoom) and the full desktop fits.
pub fn apply_viewport(vp: &Viewport) -> (u32, u32) {
    let portrait = vp.orientation.eq_ignore_ascii_case("portrait") || vp.h > vp.w;
    let pts_w = if vp.points_w > 1.0 {
        vp.points_w
    } else {
        vp.w as f32 / vp.scale.max(1.0)
    };
    let pts_h = if vp.points_h > 1.0 {
        vp.points_h
    } else {
        vp.h as f32 / vp.scale.max(1.0)
    };

    let (enc_w, enc_h) = encode_size(vp);

    if !session_live() {
        tracing::info!("no Plasma session yet; encoder will scale to {enc_w}x{enc_h}");
        return (enc_w, enc_h);
    }

    let out = match kscreen(&["-o"]) {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(e) => {
            tracing::warn!("kscreen-doctor: {e}");
            return (enc_w, enc_h);
        }
    };
    let Some(primary) = parse_primary(&out) else {
        return (enc_w, enc_h);
    };
    if SAVED.lock().unwrap().is_none() {
        let _ = save_current();
    }

    let modes = parse_modes(&out);
    // Never rotate the laptop panel — GSR captures the DRM plane as landscape.

    // Target logical pixels ≈ the longer client side so UI stays readable.
    let long_pts = pts_w.max(pts_h);
    let (target_lw, target_lh) = if portrait {
        (long_pts.max(800.0), (long_pts.max(800.0) * 10.0 / 16.0).max(500.0))
    } else {
        (pts_w.max(800.0), pts_h.max(500.0))
    };

    let mut best: Option<(String, f32, f32)> = None; // mode, scale, score
    for (mw, mh, hz, label) in &modes {
        let (lw, lh) = (*mw as f32, *mh as f32);
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0] {
            let log_w = lw / scale;
            let log_h = lh / scale;
            let score = (log_w - target_lw).abs() + (log_h - target_lh).abs() + (120.0 - hz) * 0.01;
            let better = best
                .as_ref()
                .map(|(_, _, s)| score < *s)
                .unwrap_or(true);
            if better {
                best = Some((label.clone(), scale, score));
            }
        }
    }

    if let Some((mode, scale, _)) = best {
        let spec_mode = format!("output.{}.mode.{mode}", primary.name);
        let spec_scale = format!("output.{}.scale.{scale}", primary.name);
        match kscreen(&[&spec_mode, &spec_scale]) {
            Ok(o) if o.status.success() => {
                tracing::info!("display {} -> mode {mode} scale {scale}", primary.name);
            }
            Ok(o) => tracing::warn!(
                "kscreen apply failed: {}",
                String::from_utf8_lossy(&o.stderr)
            ),
            Err(e) => tracing::warn!("kscreen apply: {e}"),
        }
    }

    (enc_w, enc_h)
}

fn parse_modes(output: &str) -> Vec<(u32, u32, f32, String)> {
    let mut modes = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if !t.starts_with("Modes:") {
            continue;
        }
        for tok in t.split_whitespace().skip(1) {
            let clean = tok.replace("!*", "").replace('*', "").replace('!', "");
            let label = clean.split(':').nth(1).unwrap_or(&clean);
            if let Some((wh, hz)) = label.split_once('@') {
                if let Some((w, h)) = wh.split_once('x') {
                    if let (Ok(w), Ok(h), Ok(hz)) =
                        (w.parse::<u32>(), h.parse::<u32>(), hz.parse::<f32>())
                    {
                        modes.push((w, h, hz, label.to_string()));
                    }
                }
            }
        }
    }
    modes
}

fn fit_even(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let mut w = w.max(2);
    let mut h = h.max(2);
    let sx = max_w as f32 / w as f32;
    let sy = max_h as f32 / h as f32;
    let s = sx.min(sy).min(1.0);
    w = ((w as f32 * s) as u32) & !1;
    h = ((h as f32 * s) as u32) & !1;
    (w.max(640), h.max(360))
}
