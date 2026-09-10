use anyhow::{bail, Context, Result};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PORT: u16 = 44789;
pub const ALPN: &[u8] = b"ddrdesk/1";
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

pub const CLIENT_HELLO: u8 = 0x01;
pub const SERVER_HELLO: u8 = 0x02;
pub const AUTH_FAIL: u8 = 0x03;
pub const VIEWPORT: u8 = 0x04;
pub const INPUT: u8 = 0x05;
pub const VIDEO: u8 = 0x06;
pub const PING: u8 = 0x07;
pub const PONG: u8 = 0x08;
pub const REQUEST_KEYFRAME: u8 = 0x09;
pub const STATUS: u8 = 0x0A;
pub const GOODBYE: u8 = 0x0B;
pub const BITRATE_HINT: u8 = 0x0C;
pub const CURSOR: u8 = 0x0D;
pub const UI_SCALE: u8 = 0x0E;

pub const VIDEO_FLAG_KEY: u8 = 0x01;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    pub id: String,
    #[serde(default)]
    pub device: String,
    pub w: u32,
    pub h: u32,
    #[serde(default)]
    pub points_w: f32,
    #[serde(default)]
    pub points_h: f32,
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default = "default_orientation")]
    pub orientation: String,
    #[serde(default)]
    pub proto: u32,
    #[serde(default)]
    pub session: Option<String>,
}

fn default_scale() -> f32 {
    2.0
}
fn default_orientation() -> String {
    "landscape".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHello {
    pub ok: bool,
    pub name: String,
    pub fp: String,
    pub endpoints: Vec<String>,
    pub screen_w: u32,
    pub screen_h: u32,
    pub session: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Viewport {
    pub w: u32,
    pub h: u32,
    #[serde(default)]
    pub points_w: f32,
    #[serde(default)]
    pub points_h: f32,
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default = "default_orientation")]
    pub orientation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum InputEvent {
    #[serde(rename = "move")]
    Move { dx: f32, dy: f32 },
    #[serde(rename = "btn")]
    Btn { b: String, d: bool },
    #[serde(rename = "wheel")]
    Wheel { dx: f32, dy: f32 },
    #[serde(rename = "key")]
    Key { k: String, d: bool },
    #[serde(rename = "text")]
    Text { s: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub state: String,
    #[serde(default)]
    pub msg: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitrateHint {
    pub kbps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPos {
    pub x: f32,
    pub y: f32,
    pub w: u32,
    pub h: u32,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiScale {
    pub factor: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthFail {
    pub error: String,
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<(u8, Bytes)> {
    let mut lenb = [0u8; 4];
    r.read_exact(&mut lenb).await.context("read length")?;
    let len = u32::from_be_bytes(lenb) as usize;
    if len == 0 || len > MAX_FRAME {
        bail!("invalid frame length {len}");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await.context("read body")?;
    Ok((buf[0], Bytes::copy_from_slice(&buf[1..])))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, typ: u8, payload: &[u8]) -> Result<()> {
    let len = (1 + payload.len()) as u32;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&[typ]).await?;
    w.write_all(payload).await?;
    w.flush().await?;
    Ok(())
}

pub fn video_payload(flags: u8, pts_us: u64, nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + nal.len());
    out.push(flags);
    out.extend_from_slice(&pts_us.to_be_bytes());
    out.extend_from_slice(nal);
    out
}

#[allow(dead_code)]
pub fn parse_video(payload: &[u8]) -> Result<(u8, u64, &[u8])> {
    if payload.len() < 9 {
        bail!("short video frame");
    }
    let flags = payload[0];
    let pts = u64::from_be_bytes(payload[1..9].try_into().unwrap());
    Ok((flags, pts, &payload[9..]))
}
