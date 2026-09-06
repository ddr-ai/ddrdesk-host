//! Streaming FLV demuxer → Annex-B H.264 NAL units.

use anyhow::{bail, Result};
use bytes::Bytes;
use std::io::{Read, Write};

#[derive(Debug, Clone)]
pub struct NalUnit {
    pub keyframe: bool,
    pub pts_us: u64,
    pub data: Bytes,
}

pub struct FlvH264 {
    sps_pps: Vec<u8>,
    nalu_size_len: usize,
}

impl Default for FlvH264 {
    fn default() -> Self {
        Self {
            sps_pps: Vec::new(),
            nalu_size_len: 4,
        }
    }
}

impl FlvH264 {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read one H.264 access unit from a blocking FLV reader. Returns None on EOF.
    pub fn next_au<R: Read>(&mut self, r: &mut R) -> Result<Option<NalUnit>> {
        loop {
            match self.read_tag(r)? {
                None => return Ok(None),
                Some(Some(au)) => return Ok(Some(au)),
                Some(None) => continue,
            }
        }
    }

    fn read_tag<R: Read>(&mut self, r: &mut R) -> Result<Option<Option<NalUnit>>> {
        let mut hdr = [0u8; 11];
        match r.read_exact(&mut hdr) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        // First 9+4 bytes of the file are FLV header + PreviousTagSize0.
        // Callers must skip the 13-byte header before using this.
        let tag_type = hdr[0];
        let data_size = u32::from_be_bytes([0, hdr[1], hdr[2], hdr[3]]) as usize;
        let ts = u32::from_be_bytes([hdr[7], hdr[4], hdr[5], hdr[6]]);
        let mut data = vec![0u8; data_size];
        r.read_exact(&mut data)?;
        let mut prev = [0u8; 4];
        r.read_exact(&mut prev)?;

        if tag_type != 9 {
            return Ok(Some(None));
        }
        if data.len() < 5 {
            return Ok(Some(None));
        }
        let codec = data[0] & 0x0f;
        let frame_type = data[0] >> 4;
        if codec != 7 {
            return Ok(Some(None));
        }
        let avc_packet_type = data[1];
        let pts_us = (ts as u64) * 1000;
        match avc_packet_type {
            0 => {
                self.parse_avcc(&data[5..])?;
                Ok(Some(None))
            }
            1 => {
                let mut annexb = Vec::new();
                let is_key = frame_type == 1;
                if is_key && !self.sps_pps.is_empty() {
                    annexb.extend_from_slice(&self.sps_pps);
                }
                avcc_to_annexb(&data[5..], self.nalu_size_len, &mut annexb)?;
                if annexb.is_empty() {
                    return Ok(Some(None));
                }
                Ok(Some(Some(NalUnit {
                    keyframe: is_key,
                    pts_us,
                    data: Bytes::from(annexb),
                })))
            }
            _ => Ok(Some(None)),
        }
    }

    fn parse_avcc(&mut self, rec: &[u8]) -> Result<()> {
        if rec.len() < 7 {
            bail!("short AVCDecoderConfigurationRecord");
        }
        self.nalu_size_len = ((rec[4] & 3) + 1) as usize;
        let mut i = 5;
        let num_sps = (rec[i] & 0x1f) as usize;
        i += 1;
        let mut out = Vec::new();
        for _ in 0..num_sps {
            if i + 2 > rec.len() {
                bail!("truncated SPS");
            }
            let n = u16::from_be_bytes([rec[i], rec[i + 1]]) as usize;
            i += 2;
            if i + n > rec.len() {
                bail!("truncated SPS body");
            }
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&rec[i..i + n]);
            i += n;
        }
        if i >= rec.len() {
            self.sps_pps = out;
            return Ok(());
        }
        let num_pps = rec[i] as usize;
        i += 1;
        for _ in 0..num_pps {
            if i + 2 > rec.len() {
                bail!("truncated PPS");
            }
            let n = u16::from_be_bytes([rec[i], rec[i + 1]]) as usize;
            i += 2;
            if i + n > rec.len() {
                bail!("truncated PPS body");
            }
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&rec[i..i + n]);
            i += n;
        }
        self.sps_pps = out;
        Ok(())
    }
}

fn avcc_to_annexb(mut src: &[u8], nalu_size_len: usize, out: &mut Vec<u8>) -> Result<()> {
    if nalu_size_len == 0 || nalu_size_len > 4 {
        bail!("bad nalu size length {nalu_size_len}");
    }
    while src.len() >= nalu_size_len {
        let mut len = 0usize;
        for k in 0..nalu_size_len {
            len = (len << 8) | src[k] as usize;
        }
        src = &src[nalu_size_len..];
        if len == 0 {
            continue;
        }
        if src.len() < len {
            bail!("truncated NALU (want {len} have {})", src.len());
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&src[..len]);
        src = &src[len..];
    }
    Ok(())
}

pub fn skip_flv_header<R: Read>(r: &mut R) -> Result<()> {
    let mut hdr = [0u8; 13];
    r.read_exact(&mut hdr)?;
    if &hdr[0..3] != b"FLV" {
        bail!("not an FLV stream");
    }
    Ok(())
}

/// Drain a writer so a FIFO open(write) unblocks. No-op helper for tests.
pub fn _touch_writer<W: Write>(w: &mut W) -> Result<()> {
    w.flush()?;
    Ok(())
}
