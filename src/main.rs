#![cfg(target_os = "linux")]

mod config;
mod device;
mod edge;
mod inject;
mod setup;
mod state;

use clap::Parser;
use device::Touchpad;
use edge::Settings;
use std::collections::BTreeMap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Instant;

#[derive(Parser)]
#[command(
    version,
    about = "Observe-only edge motion for Linux touchpads, with or without clicking"
)]
struct Args {
    /// Override the configured edge width cap in mm (default: no cap).
    #[arg(long, value_parser = positive)]
    edge_width_mm: Option<f64>,
    /// Override the configured maximum speed scale.
    #[arg(long, value_parser = positive)]
    max_speed_scale: Option<f64>,
    /// Override the configured edge dwell time in milliseconds (default: 150).
    #[arg(long)]
    dwell_ms: Option<u64>,
    /// Print eligible devices without connecting to a graphical session, then exit.
    #[arg(long)]
    list_devices: bool,
    /// Calibrate touchpad bounds and save per-axis edge percentages.
    #[arg(long, short = 's', conflicts_with = "list_devices")]
    setup: bool,
    #[arg(long)]
    verbose: bool,
    /// Log raw touchpad events, activation decisions, timer ticks and injected motion.
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

fn fd(result: libc::c_int) -> io::Result<OwnedFd> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(result) })
    }
}

struct Reactor {
    epoll: OwnedFd,
    timer: OwnedFd,
    signal: OwnedFd,
    armed: bool,
}

impl Reactor {
    fn new() -> io::Result<Self> {
        // Block termination signals before creating signalfd; no asynchronous handlers.
        let mut mask = unsafe { std::mem::zeroed::<libc::sigset_t>() };
        unsafe {
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGINT);
            libc::sigaddset(&mut mask, libc::SIGTERM);
            if libc::sigprocmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut()) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let reactor = Self {
            epoll: fd(unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) })?,
            timer: fd(unsafe {
                libc::timerfd_create(
                    libc::CLOCK_MONOTONIC,
                    libc::TFD_NONBLOCK | libc::TFD_CLOEXEC,
                )
            })?,
            signal: fd(unsafe {
                libc::signalfd(-1, &mask, libc::SFD_NONBLOCK | libc::SFD_CLOEXEC)
            })?,
            armed: false,
        };
        reactor.add(reactor.timer.as_raw_fd())?;
        reactor.add(reactor.signal.as_raw_fd())?;
        Ok(reactor)
    }

    fn add(&self, descriptor: RawFd) -> io::Result<()> {
        let mut event = libc::epoll_event {
            events: libc::EPOLLIN as u32,
            u64: descriptor as u64,
        };
        if unsafe {
            libc::epoll_ctl(
                self.epoll.as_raw_fd(),
                libc::EPOLL_CTL_ADD,
                descriptor,
                &mut event,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn remove(&self, descriptor: RawFd) {
        // Closing the descriptor also removes it; deletion is best-effort on unplug.
        unsafe {
            libc::epoll_ctl(
                self.epoll.as_raw_fd(),
                libc::EPOLL_CTL_DEL,
                descriptor,
                std::ptr::null_mut(),
            );
        }
    }

    fn arm(&mut self, active: bool) -> io::Result<()> {
        if self.armed == active {
            return Ok(());
        }
        let mut spec = unsafe { std::mem::zeroed::<libc::itimerspec>() };
        if active {
            spec.it_interval.tv_nsec = (1_000_000_000 / edge::TICK_HZ) as _;
            spec.it_value = spec.it_interval;
        }
        if unsafe { libc::timerfd_settime(self.timer.as_raw_fd(), 0, &spec, std::ptr::null_mut()) }
            < 0
        {
            return Err(io::Error::last_os_error());
        }
        self.armed = active;
        log::debug!(
            "timerfd {} ({} Hz)",
            if active { "armed" } else { "disarmed" },
            edge::TICK_HZ
        );
        Ok(())
    }

    fn consume_timer(&self) -> io::Result<()> {
        let mut expirations = 0_u64;
        let result = unsafe {
            libc::read(
                self.timer.as_raw_fd(),
                (&mut expirations as *mut u64).cast(),
                8,
            )
        };
        if result < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::WouldBlock && e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
        if result == 8 {
            log::trace!("timerfd expirations={expirations}");
        }
        Ok(())
    }
}

fn add_device(
    reactor: &Reactor,
    pads: &mut BTreeMap<RawFd, Touchpad>,
    device: &udev::Device,
    settings: Settings,
    calibrations: &[config::Calibration],
) -> Result<(), String> {
    if pads.values().any(|p| p.syspath == device.syspath()) {
        return Ok(());
    }
    if let Some(pad) = Touchpad::open(device, settings, false, calibrations)? {
        reactor
            .add(pad.fd())
            .map_err(|e| format!("epoll touchpad: {e}"))?;
        pads.insert(pad.fd(), pad);
    }
    Ok(())
}

fn refresh(
    reactor: &mut Reactor,
    pads: &mut BTreeMap<RawFd, Touchpad>,
    now: Instant,
) -> io::Result<()> {
    let mut active = false;
    for pad in pads.values_mut() {
        if pad.motion.active(now) {
            active = true;
            if pad.last_tick.is_none() {
                log::debug!(
                    "{}: motion activated velocity_px_s={:?}",
                    pad.path.display(),
                    pad.motion.velocity(now)
                );
            }
            pad.last_tick.get_or_insert(now);
        } else {
            pad.last_tick = None;
            pad.accumulator.reset();
        }
    }
    reactor.arm(active)
}

fn timeout(pads: &BTreeMap<RawFd, Touchpad>, now: Instant) -> i32 {
    // Dwell must expire even when the finger is perfectly still. Use one epoll
    // deadline, not a periodic timer; timerfd remains disarmed until motion starts.
    pads.values()
        .filter_map(|p| p.motion.deadline())
        .filter(|t| *t > now)
        .min()
        .map(|t| {
            t.duration_since(now)
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32
        })
        .unwrap_or(-1)
}

fn run(args: Args) -> Result<(), String> {
    let path = config::path()?;
    let mut config = config::load(&path)?;
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
    let settings = config.settings();
    log::debug!(
        "configuration: path={} calibrated_devices={}",
        path.display(),
        config.devices.len()
    );
    log::debug!(
        "startup: version={} pid={} uid={} euid={} settings={settings:?} list_devices={} dbg={} verbose={}",
        env!("CARGO_PKG_VERSION"), std::process::id(), unsafe { libc::getuid() },
        unsafe { libc::geteuid() }, args.list_devices, args.dbg, args.verbose
    );
    if args.list_devices {
        let mut count = 0;
        for device in device::enumerate().map_err(|e| format!("udev enumeration: {e}"))? {
            if Touchpad::open(&device, settings, true, &config.devices)?.is_some() {
                count += 1;
            }
        }
        if count == 0 {
            println!("No eligible touchpads found.");
        }
        return Ok(());
    }
    let mut reactor = Reactor::new().map_err(|e| format!("event loop setup: {e}"))?;
    // Listen before enumerating so arrivals during startup cannot be lost.
    let monitor = device::monitor().map_err(|e| format!("udev monitor: {e}"))?;
    reactor
        .add(monitor.as_raw_fd())
        .map_err(|e| format!("epoll udev: {e}"))?;
    let mut injector = inject::Injector::new()?;
    let mut pads = BTreeMap::new();
    for device in device::enumerate().map_err(|e| format!("udev enumeration: {e}"))? {
        add_device(&reactor, &mut pads, &device, settings, &config.devices)?;
    }
    if pads.is_empty() {
        log::info!("no touchpads found; waiting for hotplug");
    }
    let mut events = [libc::epoll_event { events: 0, u64: 0 }; 64];
    loop {
        let now = Instant::now();
        refresh(&mut reactor, &mut pads, now).map_err(|e| format!("timer setup: {e}"))?;
        let wait_ms = timeout(&pads, now);
        log::trace!(
            "epoll wait: touchpads={} timer_armed={} timeout_ms={wait_ms}",
            pads.len(),
            reactor.armed
        );
        let result = unsafe {
            libc::epoll_wait(
                reactor.epoll.as_raw_fd(),
                events.as_mut_ptr(),
                events.len() as _,
                wait_ms,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("epoll wait: {error}"));
        }
        let ready = &events[..result as usize];
        log::trace!("epoll wake: ready_fds={result}");
        if ready
            .iter()
            .any(|e| e.u64 == reactor.signal.as_raw_fd() as u64)
        {
            log::info!("shutting down");
            return Ok(()); // Drop the cursor connection without changing any button state.
        }
        if ready.iter().any(|e| e.u64 == monitor.as_raw_fd() as u64) {
            for event in monitor.iter() {
                log::debug!(
                    "udev: action={} syspath={} devnode={:?} ID_INPUT={:?} ID_INPUT_TOUCHPAD={:?} ID_INPUT_MOUSE={:?}",
                    event.event_type(), event.syspath().display(), event.devnode(),
                    event.property_value("ID_INPUT"), event.property_value("ID_INPUT_TOUCHPAD"),
                    event.property_value("ID_INPUT_MOUSE")
                );
                match event.event_type() {
                    udev::EventType::Add | udev::EventType::Change => {
                        add_device(&reactor, &mut pads, &event, settings, &config.devices)?
                    }
                    udev::EventType::Remove => {
                        if let Some(key) = pads
                            .iter()
                            .find_map(|(key, p)| (p.syspath == event.syspath()).then_some(*key))
                        {
                            reactor.remove(key);
                            if let Some(pad) = pads.remove(&key) {
                                log::info!("removed {}", pad.path.display());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Drain all readable physical sources before injecting: lift/multitouch
        // frames in this epoll batch always take precedence over the timer.
        for event in ready {
            let descriptor = event.u64 as RawFd;
            let flags = event.events;
            log::trace!("epoll ready: fd={descriptor} flags={flags:#x}");
            if let Some(pad) = pads.get_mut(&descriptor) {
                let alive = pad
                    .drain()
                    .map_err(|e| device::access_error(&pad.path, e))?;
                if !alive || event.events & (libc::EPOLLHUP | libc::EPOLLERR) as u32 != 0 {
                    log::debug!(
                        "{}: evdev disconnected alive={alive} epoll_flags={flags:#x}",
                        pad.path.display()
                    );
                    reactor.remove(descriptor);
                    pads.remove(&descriptor);
                }
            }
        }
        let now = Instant::now();
        refresh(&mut reactor, &mut pads, now).map_err(|e| format!("timer setup: {e}"))?;
        if ready
            .iter()
            .any(|e| e.u64 == reactor.timer.as_raw_fd() as u64)
        {
            reactor
                .consume_timer()
                .map_err(|e| format!("timer read: {e}"))?;
            let mut total = [0_i32; 2];
            for pad in pads.values_mut() {
                if let Some(previous) = pad.last_tick.replace(now) {
                    if pad.motion.active(now) {
                        let dt = now.duration_since(previous);
                        let velocity = pad.motion.velocity(now);
                        let delta = pad.accumulator.step(velocity, dt);
                        log::trace!(
                            "{}: tick dt_ms={:.3} velocity_px_s={velocity:?} delta_px={delta:?}",
                            pad.path.display(),
                            dt.as_secs_f64() * 1000.0
                        );
                        total[0] = total[0].saturating_add(delta[0]);
                        total[1] = total[1].saturating_add(delta[1]);
                    } else {
                        pad.last_tick = None;
                    }
                } else {
                    pad.last_tick = None;
                }
            }
            injector
                .emit(total)
                .map_err(|e| format!("Enigo cursor movement: {e}"))?;
        }
    }
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    let mut logger = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(if args.verbose { "debug" } else { "info" }),
    );
    if args.dbg {
        // An explicit debug request wins over RUST_LOG for this binary only.
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
