#![cfg(target_os = "linux")]

mod config;
mod device;
mod edge;
mod inject;
mod protocol;
mod service;
mod session;
mod setup;
mod state;

use clap::Parser;
use device::Touchpad;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Privilege-isolated touchpad edge motion for Linux")]
struct Args {
    /// Configuration file (root defaults to /etc/e-motional-lib/config.toml).
    #[arg(long)]
    config_path: Option<PathBuf>,
    /// Override the edge width cap in mm during setup or device listing.
    #[arg(long, value_parser = positive)]
    edge_width_mm: Option<f64>,
    /// Override the maximum speed scale during setup or device listing.
    #[arg(long, value_parser = positive)]
    max_speed_scale: Option<f64>,
    /// Override the edge dwell time in milliseconds during setup or device listing.
    #[arg(long)]
    dwell_ms: Option<u64>,
    /// Print eligible touchpads without creating a virtual pointer.
    #[arg(long)]
    list_devices: bool,
    /// Select and calibrate a touchpad, then save its configuration.
    #[arg(long, short = 's', conflicts_with = "list_devices")]
    setup: bool,
    #[arg(long, hide = true, conflicts_with_all = ["worker", "setup", "list_devices"])]
    broker: bool,
    #[arg(long, hide = true, conflicts_with_all = ["broker", "setup", "list_devices"])]
    worker: bool,
    #[arg(long)]
    verbose: bool,
    /// Enable sensitive raw-input and motion trace logging.
    #[arg(long, short = 'd')]
    dbg: bool,
}

fn positive(text: &str) -> Result<f64, String> {
    let number: f64 = text
        .parse()
        .map_err(|_| "expected a positive finite number")?;
    if number.is_finite() && number > 0.0 && number <= 1_000_000.0 {
        Ok(number)
    } else {
        Err("expected a positive finite number <= 1000000".into())
    }
}

fn run(args: Args) -> Result<(), String> {
    let administrative = args.setup || args.list_devices;
    if !administrative
        && (args.edge_width_mm.is_some()
            || args.max_speed_scale.is_some()
            || args.dwell_ms.is_some()
            || (!args.broker && args.config_path.is_some()))
    {
        return Err("service settings come from the root-owned configuration; use --setup to change them".into());
    }
    // The isolated worker never opens the host configuration or physical devices.
    if args.worker || (!args.broker && !administrative) {
        return service::worker();
    }
    let path = args.config_path.map_or_else(config::path, Ok)?;
    if unsafe { libc::geteuid() } == 0 {
        if path != std::path::Path::new(config::SYSTEM_PATH) {
            return Err(format!("privileged operation requires --config-path {}", config::SYSTEM_PATH));
        }
        if args.setup || args.broker {
            config::check_system_path(args.setup)?;
        }
    } else if args.broker {
        return Err("the device broker must be started by the system service manager as root".into());
    }
    let _configuration_lock = if unsafe { libc::geteuid() } == 0 && (args.setup || args.broker) {
        let directory = std::fs::File::open(std::path::Path::new(config::SYSTEM_PATH).parent().unwrap())
            .map_err(|error| format!("cannot open configuration directory: {error}"))?;
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Err("configuration is in use; stop e-motional-lib.service and e-motional-lib-broker.service before setup or restart".into());
        }
        Some(directory)
    } else {
        None
    };
    let mut config = config::load(&path)?;
    if args.broker {
        if config.devices.is_empty() {
            return Err("no touchpads configured; run sudo e-motional-lib --setup first".into());
        }
        return service::broker(&config);
    }
    if let Some(width) = args.edge_width_mm {
        config.edge_width_mm = Some(width);
    }
    if let Some(scale) = args.max_speed_scale {
        config.max_speed_scale = scale;
    }
    if let Some(dwell) = args.dwell_ms {
        config.dwell_ms = dwell;
    }
    config.validate()?;
    if args.setup {
        return setup::run(&path, &mut config);
    }
    let mut count = 0;
    for device in device::enumerate().map_err(|error| format!("udev enumeration: {error}"))? {
        if Touchpad::open(&device, config.settings(), true, &config.devices)?.is_some() {
            count += 1;
        }
    }
    if count == 0 {
        println!("No eligible touchpads found.");
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    let mut logger = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(if args.verbose { "debug" } else { "info" }),
    );
    if args.dbg {
        logger
            .filter_module(env!("CARGO_CRATE_NAME"), log::LevelFilter::Trace)
            .format_timestamp_millis();
    } else {
        logger.format_timestamp(None);
    }
    logger.init();
    match run(args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            log::error!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
