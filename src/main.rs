mod capture;
mod discover;
mod display;
mod flv;
mod id;
mod input;
mod net;
mod proto;
mod status;
mod stun;
mod tls;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use crate::discover::{local_ips, try_upnp, Discovery};
use crate::id::{format_id, load_or_create_id, regen_id};
use crate::net::HostState;
use crate::proto::PORT;

#[derive(Parser, Debug)]
#[command(name = "ddrdeskd", about = "DDRDesk host daemon")]
struct Args {
    /// Print the 9-digit connection ID and exit.
    #[arg(long)]
    print_id: bool,
    /// Generate a new connection ID and print it.
    #[arg(long)]
    regen_id: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    if args.regen_id {
        let id = regen_id()?;
        println!("{}", format_id(&id));
        return Ok(());
    }
    let id = load_or_create_id()?;
    if args.print_id {
        println!("{}", format_id(&id));
        return Ok(());
    }

    let tls = tls::load_or_create()?;
    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "ddrdesk".into());

    let mut endpoints: Vec<String> = local_ips()
        .into_iter()
        .map(|ip| match ip {
            std::net::IpAddr::V6(v) => format!("[{v}]:{PORT}"),
            other => format!("{other}:{PORT}"),
        })
        .collect();

    if let Some(upnp) = try_upnp(PORT).await {
        if !endpoints.contains(&upnp) {
            endpoints.push(upnp);
        }
    }
    match stun::public_addr(PORT).await {
        Ok(sa) => {
            let ep = sa.to_string();
            tracing::info!("STUN public address hint {ep}");
            if !endpoints.contains(&ep) {
                endpoints.push(ep);
            }
        }
        Err(e) => tracing::info!("STUN skipped: {e}"),
    }

    let _mdns = match Discovery::start(&id, &local_ips()) {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::warn!("mDNS failed: {e:#}");
            None
        }
    };

    tracing::info!("DDRDesk host ready");
    tracing::info!("connection ID: {}", format_id(&id));
    tracing::info!("endpoints: {}", endpoints.join(", "));
    tracing::info!("tls {}", tls.fingerprint);

    crate::status::write(&id, &hostname, &endpoints, &tls.fingerprint);

    let state = Arc::new(HostState {
        id,
        tls,
        endpoints,
        hostname,
    });

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        r = net::serve(state) => r?,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("SIGINT, restoring display");
        }
        _ = sigterm.recv() => {
            tracing::info!("SIGTERM, restoring display");
        }
    }
    display::restore();
    Ok(())
}
