use assi_relay::{Identity, Limits, Server};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("assi-relay: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut bind: SocketAddr = "127.0.0.1:4788".parse()?;
    let mut advertised: Option<IpAddr> = None;
    let mut identity = PathBuf::from("assi-relay-identity.json");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("assi-relay [--bind IP:PORT] [--advertise IP] [--identity FILE]\nDefault: loopback 127.0.0.1:4788. Private identity file is created only if absent.\nThe printed assi-relay:// configuration contains the public certificate, never the private key.\nTo expose a server, explicitly supply --bind and a reachable --advertise address.\nNo firewall or network rules are modified.");
                return Ok(());
            }
            "--bind" => bind = args.next().ok_or("--bind needs IP:PORT")?.parse()?,
            "--advertise" => advertised = Some(args.next().ok_or("--advertise needs IP")?.parse()?),
            "--identity" => {
                identity = PathBuf::from(args.next().ok_or("--identity needs a file path")?)
            }
            _ => return Err("Unknown argument; use --help".into()),
        }
    }
    let advertised = advertised.unwrap_or(bind.ip());
    let server = Server::bind(
        bind,
        advertised,
        Identity::load_or_create(&identity)?,
        Limits::default(),
    )
    .await?;
    println!("{}", server.config().encode()?);
    tokio::signal::ctrl_c().await?;
    server.shutdown().await?;
    Ok(())
}
