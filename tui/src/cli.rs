use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Clone, Parser)]
#[command(name = env!("CARGO_PKG_NAME"))]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = env!("CARGO_PKG_DESCRIPTION"))]
#[command(author = env!("CARGO_PKG_AUTHORS"))]
pub struct Cli {
    /// Set log level
    #[arg(long, short = 'L', env = "RUST_LOG", default_value_t = String::from(concat!(env!("CARGO_PKG_NAME"), "=info,warn")))]
    pub logging: String,

    /// Custom config path
    #[arg(long, short = 'c', env = "RPWS_CONFIG")]
    pub config: Option<PathBuf>,

    /// Overide API address
    #[arg(long, short = 'a', env = "API_ADDR")]
    pub api_addr: Option<String>,

    #[clap(subcommand)]
    pub mode: Option<Mode>,
}

#[derive(Subcommand, Clone)]
pub enum Mode {
    /// Launch daemon
    Daemon {
        /// Don't connect to global IPFS DHT (good for local testing)
        #[arg(long, short = 'l')]
        local: bool,
    },
}
