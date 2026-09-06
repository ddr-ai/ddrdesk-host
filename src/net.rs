use anyhow::{Context, Result};
use constant_time_eq::constant_time_eq;
use rand::RngCore;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::split;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;
use tokio_rustls::server::TlsStream;

use crate::capture::{Capture, CaptureParams};
use crate::display;
use crate::flv::NalUnit;
use crate::id::format_id;
use crate::input::Injector;
use crate::proto::{
    self, AuthFail, BitrateHint, ClientHello, InputEvent, ServerHello, Status, Viewport,
    VIDEO_FLAG_KEY,
};
use crate::tls::Tls;

struct RateLimit {
    hits: HashMap<IpAddrKey, (u32, Instant)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct IpAddrKey(u128);

impl RateLimit {
    fn new() -> Self {
        Self {
            hits: HashMap::new(),
        }
    }
    fn allow(&mut self, addr: SocketAddr) -> bool {
        let key = match addr.ip() {
            std::net::IpAddr::V4(v) => IpAddrKey(u32::from(v) as u128),
            std::net::IpAddr::V6(v) => IpAddrKey(u128::from(v)),
        };
        let now = Instant::now();
        self.hits.retain(|_, (_, t)| now.duration_since(*t) < Duration::from_secs(60));
        let e = self.hits.entry(key).or_insert((0, now));
        if now.duration_since(e.1) > Duration::from_secs(30) {
            *e = (0, now);
        }
        e.0 += 1;
        e.0 <= 8
    }
}

pub struct HostState {
    pub id: String,
    pub tls: Tls,
    pub endpoints: Vec<String>,
    pub hostname: String,
}

pub async fn serve(state: Arc<HostState>) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", proto::PORT))
        .await
        .with_context(|| format!("bind 0.0.0.0:{}", proto::PORT))?;
    tracing::info!("listening on 0.0.0.0:{}", proto::PORT);
    let active: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> = Arc::new(Mutex::new(None));
    let rate = Arc::new(Mutex::new(RateLimit::new()));

    loop {
        let (tcp, addr) = listener.accept().await?;
        let _ = tcp.set_nodelay(true);
        let state = state.clone();
        let active = active.clone();
        let rate = rate.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(tcp, addr, state, active, rate).await {
                tracing::warn!("client {addr}: {e:#}");
            }
        });
    }
}

async fn handle_conn(
    tcp: TcpStream,
    addr: SocketAddr,
    state: Arc<HostState>,
    active: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    rate: Arc<Mutex<RateLimit>>,
) -> Result<()> {
    tracing::info!("incoming {addr}");
    let tls = match state.tls.acceptor.accept(tcp).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("tls handshake {addr}: {e}");
            return Ok(());
        }
    };
    run_session(tls, addr, state, active, rate).await
}

async fn run_session(
    stream: TlsStream<TcpStream>,
    addr: SocketAddr,
    state: Arc<HostState>,
    _active: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    rate: Arc<Mutex<RateLimit>>,
) -> Result<()> {
    let (mut rd, mut wr) = split(stream);
    let (typ, payload) = proto::read_frame(&mut rd).await?;
    if typ != proto::CLIENT_HELLO {
        anyhow::bail!("expected ClientHello");
    }
    let hello: ClientHello = serde_json::from_slice(&payload)?;
    let id_ok = constant_time_eq(hello.id.as_bytes(), state.id.as_bytes());
    if !id_ok {
        if !rate.lock().await.allow(addr) {
            let fail = AuthFail {
                error: "rate_limited".into(),
            };
            let _ = proto::write_frame(&mut wr, proto::AUTH_FAIL, &serde_json::to_vec(&fail)?).await;
            anyhow::bail!("rate limited {addr}");
        }
        let fail = AuthFail {
            error: "bad_id".into(),
        };
        proto::write_frame(&mut wr, proto::AUTH_FAIL, &serde_json::to_vec(&fail)?).await?;
        anyhow::bail!("bad id from {addr}");
    }

    let mut session = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut session);
    let session_hex = hex::encode(session);

    let vp = Viewport {
        w: hello.w,
        h: hello.h,
        points_w: hello.points_w,
        points_h: hello.points_h,
        scale: hello.scale,
        orientation: hello.orientation.clone(),
    };
    let (enc_w, enc_h) = display::apply_viewport(&vp);

    let sh = ServerHello {
        ok: true,
        name: state.hostname.clone(),
        fp: state.tls.fingerprint.clone(),
        endpoints: state.endpoints.clone(),
        screen_w: enc_w,
        screen_h: enc_h,
        session: session_hex,
    };
    proto::write_frame(&mut wr, proto::SERVER_HELLO, &serde_json::to_vec(&sh)?).await?;
    proto::write_frame(
        &mut wr,
        proto::STATUS,
        &serde_json::to_vec(&Status {
            state: "streaming".into(),
            msg: format!("connected to {}", format_id(&state.id)),
        })?,
    )
    .await?;

    let mut injector = match Injector::new() {
        Ok(i) => Some(i),
        Err(e) => {
            tracing::error!("uinput: {e:#}");
            None
        }
    };

    let (vtx, mut vrx) = mpsc::channel::<NalUnit>(8);
    let mut params = CaptureParams {
        width: enc_w,
        height: enc_h,
        fps: 30,
        kbps: 8000,
        monitor: crate::capture::detect_monitor(),
    };
    let mut capture = Capture::start(&params, vtx.clone())?;

    let mut pointer_scale = pointer_scale_for(&vp, enc_w, enc_h);

    loop {
        tokio::select! {
            biased;
            frame = proto::read_frame(&mut rd) => {
                let (typ, payload) = match frame {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::info!("client {addr} disconnected: {e:#}");
                        break;
                    }
                };
                match typ {
                    proto::VIEWPORT => {
                        let vp: Viewport = serde_json::from_slice(&payload)?;
                        tracing::info!("viewport {}x{} {}", vp.w, vp.h, vp.orientation);
                        let (w, h) = display::apply_viewport(&vp);
                        pointer_scale = pointer_scale_for(&vp, w, h);
                        params.width = w;
                        params.height = h;
                        capture.stop();
                        let (nvtx, nvrx) = mpsc::channel::<NalUnit>(8);
                        vrx = nvrx;
                        capture = Capture::start(&params, nvtx)?;
                        let hello_update = ServerHello {
                            ok: true,
                            name: state.hostname.clone(),
                            fp: state.tls.fingerprint.clone(),
                            endpoints: state.endpoints.clone(),
                            screen_w: w,
                            screen_h: h,
                            session: sh.session.clone(),
                        };
                        proto::write_frame(&mut wr, proto::SERVER_HELLO, &serde_json::to_vec(&hello_update)?).await?;
                    }
                    proto::INPUT => {
                        let ev: InputEvent = serde_json::from_slice(&payload)?;
                        if let Some(inj) = injector.as_mut() {
                            apply_input(inj, ev, pointer_scale);
                        }
                    }
                    proto::PING => {
                        if payload.len() >= 8 {
                            let mut pong = Vec::with_capacity(16);
                            pong.extend_from_slice(&payload[..8]);
                            let now = now_us().to_be_bytes();
                            pong.extend_from_slice(&now);
                            proto::write_frame(&mut wr, proto::PONG, &pong).await?;
                        }
                    }
                    proto::REQUEST_KEYFRAME => {
                        // Next GSR keyint (≤ 0.5s) will be a keyframe; nothing extra to do.
                    }
                    proto::BITRATE_HINT => {
                        if let Ok(h) = serde_json::from_slice::<BitrateHint>(&payload) {
                            let kbps = h.kbps.clamp(800, 25000);
                            if kbps.abs_diff(params.kbps) > 500 {
                                params.kbps = kbps;
                                capture.stop();
                                let (nvtx, nvrx) = mpsc::channel::<NalUnit>(8);
                                vrx = nvrx;
                                capture = Capture::start(&params, nvtx)?;
                            }
                        }
                    }
                    proto::GOODBYE => break,
                    _ => {}
                }
            }
            au = vrx.recv() => {
                match au {
                    Some(au) => {
                        let flags = if au.keyframe { VIDEO_FLAG_KEY } else { 0 };
                        let payload = proto::video_payload(flags, au.pts_us, &au.data);
                        if proto::write_frame(&mut wr, proto::VIDEO, &payload).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        sleep(Duration::from_millis(20)).await;
                    }
                }
            }
        }
    }

    capture.stop();
    display::restore();
    Ok(())
}

fn pointer_scale_for(vp: &Viewport, enc_w: u32, _enc_h: u32) -> f32 {
    let pts_w = if vp.points_w > 1.0 {
        vp.points_w
    } else {
        vp.w as f32 / vp.scale.max(1.0)
    };
    if pts_w > 1.0 {
        enc_w as f32 / pts_w
    } else {
        1.5
    }
    .clamp(0.4, 6.0)
}

fn apply_input(inj: &mut Injector, ev: InputEvent, pointer_scale: f32) {
    match ev {
        InputEvent::Move { dx, dy } => {
            let mx = (dx * pointer_scale).round() as i32;
            let my = (dy * pointer_scale).round() as i32;
            let _ = inj.move_rel(mx, my);
        }
        InputEvent::Btn { b, d } => {
            let _ = inj.button(&b, d);
        }
        InputEvent::Wheel { dx, dy } => {
            let _ = inj.wheel(dx.round() as i32, dy.round() as i32);
        }
        InputEvent::Key { k, d } => {
            let _ = inj.key(&k, d);
        }
        InputEvent::Text { s } => {
            let _ = inj.text(&s);
        }
    }
}

fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}
