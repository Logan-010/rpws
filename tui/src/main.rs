mod cli;
mod config;
mod net;
mod ui;

use clap::Parser;
use cli::Cli;
use color_eyre::eyre::bail;
use config::Config;
use directories::ProjectDirs;
use net::Net;
use tokio::{fs, select, signal, task};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();
    let logs = ui::LogOutput::default();
    let log_writer = if cli.headless {
        tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stdout)
    } else {
        tracing_subscriber::fmt::writer::BoxMakeWriter::new(logs.clone())
    };

    if let Err(e) = tracing_subscriber::registry()
        .with(EnvFilter::new(&cli.logging))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(cli.headless)
                .with_writer(log_writer),
        )
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

    if config.secret() == Config::default().secret() {
        bail!("Change the secret in config ({})!", config_path.display());
    }

    let mut net = Net::new(&config).await?;

    net.bootstrap(cli.local, config.secret().as_bytes()).await?;
    net.cleanup();

    if !cli.headless {
        let api = net.api();
        let task = task::spawn(net.run());

        let ui_result = ui::run(api, logs);
        task.abort();
        let _ = task.await;
        ui_result?;
    } else {
        select! {
            exit_res = signal::ctrl_c() => exit_res?,
            net_res = net.run() => net_res?
        }
    }

    Ok(())
}
