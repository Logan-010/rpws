mod cli;
mod config;
mod net;
mod ui;

use clap::Parser;
use cli::{Cli, Mode};
use color_eyre::eyre::ContextCompat;
use config::Config;
use directories::ProjectDirs;
use net::{Net, api::NetApi};
use std::{collections::HashMap, time::SystemTime};
use tokio::{fs, select, signal, task};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();

    if let Err(e) = tracing_subscriber::registry()
        .with(EnvFilter::new(&cli.logging))
        .with(tracing_subscriber::fmt::layer())
        .try_init()
    {
        eprintln!("Failed to initialize logger: {e:?}");
    }

    let config_path = match cli.config {
        Some(p) => p,
        None => ProjectDirs::from("com", "Logan-010", "rpws")
            .expect("Valid project dir")
            .config_dir()
            .join("config.toml"),
    };
    let config = if fs::try_exists(&config_path).await? {
        let d = fs::read_to_string(&config_path).await?;
        Config::decode(d)?
    } else {
        if let Some(p) = config_path.parent()
            && !fs::try_exists(p).await?
        {
            fs::create_dir_all(p).await?;
        }

        let c = Config::new();

        fs::write(&config_path, c.encode()?).await?;

        c
    };

    match cli.mode {
        Some(Mode::Daemon { local }) => {
            let mut net = Net::new(&config).await?;

            let api = task::spawn(net::api::task(config.api_address(), net.api()));

            select! {
                exit_res = signal::ctrl_c() => exit_res?,
                api_res = api => api_res??,
                net_res = net.run(local, config.secret()?) => net_res?,
            }
        }
        Some(Mode::List { minify }) => {
            let addr = cli
                .api_addr
                .unwrap_or_else(|| format!("http://{}", config.api_address()));
            let api = NetApi::new(addr);

            api.connect().await?;

            let list = api.list().await?;

            let out = if minify {
                serde_json::to_string(&list)?
            } else {
                serde_json::to_string_pretty(&list)?
            };

            println!("{out}");
        }
        Some(Mode::Get { name, minify }) => {
            let addr = cli
                .api_addr
                .unwrap_or_else(|| format!("http://{}", config.api_address()));
            let api = NetApi::new(addr);

            api.connect().await?;

            let ent = api.get(&name).await?.context("No entry found")?;

            let mut entry = HashMap::new();

            entry.insert(name, ent);

            let out = if minify {
                serde_json::to_string(&entry)?
            } else {
                serde_json::to_string_pretty(&entry)?
            };

            println!("{out}");
        }
        Some(Mode::Put {
            name,
            url,
            username,
            password,
            extra,
        }) => {
            let timestamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_secs();

            let payload = shared::RecordPayload {
                url: url.unwrap_or_default(),
                username: username.unwrap_or_default(),
                password: password.unwrap_or_default(),
                timestamp,
                extra: extra.unwrap_or_default(),
            };

            let addr = cli
                .api_addr
                .unwrap_or_else(|| format!("http://{}", config.api_address()));
            let api = NetApi::new(addr);

            api.connect().await?;

            api.put(name, payload).await?;
        }
        None => {
            let addr = cli
                .api_addr
                .unwrap_or_else(|| format!("http://{}", config.api_address()));
            let api = NetApi::new(addr);

            api.connect().await?;

            ui::run(api)?;
        }
    }

    Ok(())
}
