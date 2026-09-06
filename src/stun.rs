use anyhow::{bail, Result};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{timeout, Duration};

/// Query a STUN server for the host's public UDP address. Used only as an
/// address hint stored on the client after pairing — never as a relay.
pub async fn public_addr(local_port: u16) -> Result<SocketAddr> {
    let sock = UdpSocket::bind(("0.0.0.0", 0)).await?;
    sock.connect("stun.l.google.com:19302").await?;

    let mut tx = [0u8; 20];
    tx[0] = 0x00;
    tx[1] = 0x01; // Binding request
    tx[4] = 0x21;
    tx[5] = 0x12;
    tx[6] = 0xa4;
    tx[7] = 0x42;
    for b in tx[8..].iter_mut() {
        *b = rand::random();
    }
    sock.send(&tx).await?;

    let mut rx = [0u8; 512];
    let n = timeout(Duration::from_secs(3), sock.recv(&mut rx)).await??;
    parse_xor_mapped(&rx[..n], local_port)
}

fn parse_xor_mapped(msg: &[u8], fallback_port: u16) -> Result<SocketAddr> {
    if msg.len() < 20 {
        bail!("short STUN");
    }
    let magic = [0x21u8, 0x12, 0xa4, 0x42];
    let mut i = 20;
    while i + 4 <= msg.len() {
        let atype = u16::from_be_bytes([msg[i], msg[i + 1]]);
        let alen = u16::from_be_bytes([msg[i + 2], msg[i + 3]]) as usize;
        i += 4;
        if i + alen > msg.len() {
            break;
        }
        let attr = &msg[i..i + alen];
        // XOR-MAPPED-ADDRESS 0x0020, MAPPED-ADDRESS 0x0001
        if (atype == 0x0020 || atype == 0x0001) && attr.len() >= 8 {
            let family = attr[1];
            let mut port = u16::from_be_bytes([attr[2], attr[3]]);
            if atype == 0x0020 {
                port ^= u16::from_be_bytes([magic[0], magic[1]]);
            }
            if family == 0x01 {
                let mut ip = [attr[4], attr[5], attr[6], attr[7]];
                if atype == 0x0020 {
                    for (b, m) in ip.iter_mut().zip(magic) {
                        *b ^= m;
                    }
                }
                // STUN gives the UDP mapped port; TCP UPnP mapping uses our listen port.
                let _ = port;
                return Ok(SocketAddr::from((ip, fallback_port)));
            }
        }
        i += (alen + 3) & !3;
    }
    bail!("no mapped address in STUN response")
}
