use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;

use crate::proto::PORT;

pub struct Discovery {
    mdns: ServiceDaemon,
}

impl Discovery {
    pub fn start(id: &str, extra_ips: &[IpAddr]) -> Result<Self> {
        let mdns = ServiceDaemon::new()?;
        let host = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "ddrdesk-host".into());
        let host_local = if host.ends_with(".local.") {
            host.clone()
        } else {
            format!("{host}.local.")
        };

        let mut props = HashMap::new();
        props.insert("id".to_string(), id.to_string());
        props.insert("ver".to_string(), "1".to_string());
        props.insert("name".to_string(), host);

        let svc = ServiceInfo::new(
            "_ddrdesk._tcp.local.",
            "DDRDesk",
            &host_local,
            "",
            PORT,
            Some(props),
        )?
        .enable_addr_auto();
        let _ = extra_ips;

        mdns.register(svc)?;
        tracing::info!("mDNS advertised _ddrdesk._tcp id={id}");
        Ok(Self { mdns })
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.mdns.shutdown();
    }
}

pub fn local_ips() -> Vec<IpAddr> {
    let mut out = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            let ip = iface.ip();
            match ip {
                IpAddr::V4(v) if !v.is_link_local() => out.push(ip),
                IpAddr::V6(v)
                    if !v.is_unicast_link_local() && !v.is_multicast() && !v.is_unique_local() =>
                {
                    out.push(ip)
                }
                _ => {}
            }
        }
    }
    out
}

pub async fn try_upnp(port: u16) -> Option<String> {
    match igd_next::aio::tokio::search_gateway(Default::default()).await {
        Ok(gw) => {
            let local = match local_ips()
                .into_iter()
                .find(|i| matches!(i, IpAddr::V4(_)))
            {
                Some(IpAddr::V4(v)) => v,
                _ => {
                    tracing::warn!("UPnP: no IPv4 address");
                    return None;
                }
            };
            match gw
                .add_port(
                    igd_next::PortMappingProtocol::TCP,
                    port,
                    (local, port).into(),
                    0,
                    "DDRDesk",
                )
                .await
            {
                Ok(()) => match gw.get_external_ip().await {
                    Ok(ext) => {
                        let ep = format!("{ext}:{port}");
                        tracing::info!("UPnP mapped {ep} -> {local}:{port}");
                        Some(ep)
                    }
                    Err(e) => {
                        tracing::warn!("UPnP mapped but no external IP: {e}");
                        Some(format!("{local}:{port}"))
                    }
                },
                Err(e) => {
                    tracing::warn!("UPnP add_port failed: {e}");
                    None
                }
            }
        }
        Err(e) => {
            tracing::info!("UPnP gateway not found ({e})");
            None
        }
    }
}
