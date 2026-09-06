use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::sync::mpsc;

use crate::flv::{skip_flv_header, FlvH264, NalUnit};
use crate::id::data_dir;

pub struct Capture {
    child: Option<Child>,
    /// Extra FIFO write end. Dropping this after GSR exits unblocks the reader.
    keep_open: Option<File>,
    fifo: PathBuf,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug)]
pub struct CaptureParams {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub kbps: u32,
    pub monitor: String,
}

impl Default for CaptureParams {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            fps: 30,
            kbps: 8000,
            monitor: detect_monitor(),
        }
    }
}

pub fn detect_monitor() -> String {
    let out = Command::new("gpu-screen-recorder")
        .arg("--list-monitors")
        .output()
        .ok();
    if let Some(o) = out {
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines() {
            let name = line.split('|').next().unwrap_or("").trim();
            if !name.is_empty() && name != "region" && name != "portal" && !name.starts_with("/dev/")
            {
                return name.to_string();
            }
        }
    }
    "eDP-1".into()
}

impl Capture {
    pub fn start(params: &CaptureParams, tx: mpsc::Sender<NalUnit>) -> Result<Self> {
        which_gsr()?;
        let dir = data_dir();
        crate::id::ensure_dir(&dir)?;
        let fifo = dir.join("capture.flv");
        let _ = std::fs::remove_file(&fifo);
        nix_mkfifo(&fifo)?;

        // Hold a write end so the reader can open O_RDONLY without blocking, and so
        // we can drop it on stop to deliver EOF (GSR also writes; if we opened RDWR
        // in the reader, killing GSR never unblocked read — black screen).
        let keep_open = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&fifo)
            .with_context(|| format!("open fifo keeper {}", fifo.display()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let fifo_reader = fifo.clone();
        let stop_r = stop.clone();
        let reader = std::thread::Builder::new()
            .name("ddrdesk-flv".into())
            .spawn(move || {
                if let Err(e) = read_loop(&fifo_reader, tx, stop_r) {
                    tracing::warn!("capture reader: {e:#}");
                }
            })?;

        let child = spawn_gsr(params, &fifo)?;
        Ok(Self {
            child: Some(child),
            keep_open: Some(keep_open),
            fifo,
            stop,
            reader: Some(reader),
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        // Unblock the reader: drop our FIFO write end so read() sees EOF.
        drop(self.keep_open.take());
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_file(&self.fifo);
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn which_gsr() -> Result<()> {
    which("gpu-screen-recorder").context("gpu-screen-recorder is not installed")?;
    Ok(())
}

fn which(bin: &str) -> Result<PathBuf> {
    if let Ok(p) = Command::new("which").arg(bin).output() {
        if p.status.success() {
            let s = String::from_utf8_lossy(&p.stdout).trim().to_string();
            if !s.is_empty() {
                return Ok(PathBuf::from(s));
            }
        }
    }
    bail!("{bin} not found in PATH");
}

fn nix_mkfifo(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let cpath = std::ffi::CString::new(path.to_string_lossy().as_bytes())?;
    let rc = unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) };
    if rc != 0 {
        let e = std::io::Error::last_os_error();
        bail!("mkfifo {}: {e}", path.display());
    }
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    Ok(())
}

fn spawn_gsr(params: &CaptureParams, fifo: &Path) -> Result<Child> {
    let size = format!("{}x{}", params.width.max(2) & !1, params.height.max(2) & !1);
    let kbps = params.kbps.to_string();
    let fps = params.fps.to_string();
    tracing::info!(
        "starting gpu-screen-recorder {} {} @ {} fps {} kbps -> {}",
        params.monitor,
        size,
        fps,
        kbps,
        fifo.display()
    );
    let mut child = Command::new("gpu-screen-recorder")
        .args([
            "-w",
            &params.monitor,
            "-c",
            "flv",
            "-k",
            "h264",
            "-f",
            &fps,
            "-s",
            &size,
            "-cursor",
            "yes",
            "-keyint",
            "15",
            "-encoder",
            "gpu",
            "-fallback-cpu-encoding",
            "yes",
            "-bm",
            "cbr",
            "-q",
            &kbps,
            "-tune",
            "performance",
            "-o",
            fifo.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn gpu-screen-recorder")?;

    if let Some(stderr) = child.stderr.take() {
        std::thread::Builder::new()
            .name("ddrdesk-gsr-err".into())
            .spawn(move || {
                let r = BufReader::new(stderr);
                for line in r.lines().map_while(Result::ok) {
                    let l = line.to_ascii_lowercase();
                    if l.contains("error") || l.contains("fail") {
                        tracing::warn!("gsr: {line}");
                    } else {
                        tracing::debug!("gsr: {line}");
                    }
                }
            })
            .ok();
    }
    Ok(child)
}

fn read_loop(fifo: &Path, tx: mpsc::Sender<NalUnit>, stop: Arc<AtomicBool>) -> Result<()> {
    let file: File = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(fifo)
        .with_context(|| format!("open fifo {}", fifo.display()))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    skip_flv_header(&mut reader)?;
    tracing::info!("FLV header ok, demuxing H.264");

    let mut demux = FlvH264::new();
    let mut n = 0u64;
    while !stop.load(Ordering::SeqCst) {
        match demux.next_au(&mut reader) {
            Ok(Some(au)) => {
                n += 1;
                if n <= 3 || n % 120 == 0 {
                    tracing::info!(
                        "capture frame #{n} key={} bytes={}",
                        au.keyframe,
                        au.data.len()
                    );
                }
                let is_key = au.keyframe;
                let send_ok = if is_key {
                    tx.blocking_send(au).is_ok()
                } else {
                    tx.try_send(au).is_ok()
                };
                if !send_ok && n <= 8 {
                    tracing::warn!("video queue full, dropping frame #{n}");
                }
            }
            Ok(None) => {
                tracing::info!("capture EOF after {n} frames");
                break;
            }
            Err(e) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                tracing::warn!("flv after {n} frames: {e:#}");
                break;
            }
        }
    }
    Ok(())
}
