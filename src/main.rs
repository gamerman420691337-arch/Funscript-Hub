//! Pulsar process composition. Product logic belongs to the four workspace libraries.
//! Historical attribution and the pre-engine source are retained during migration.

use anyhow::{bail, Context, Result};
use pulsar_engine::EngineConfig;
use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn main() {
    if let Err(error) = run() {
        eprintln!("pulsar: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<OsString> = std::env::args_os().collect();
    match args.get(1).and_then(|arg| arg.to_str()) {
        Some("engine") | Some("__pulsar-engine") => {
            if args.len() != 2 {
                bail!("engine role accepts no user commands; use PULSAR_STATE_DIR for an isolated state directory");
            }
            return pulsar_engine::run_server(EngineConfig::for_user()?);
        }
        Some("worker") | Some("__pulsar-worker") => {
            if args.len() != 2 {
                bail!("worker role reads one bounded engine manifest from standard input");
            }
            return pulsar_engine::worker::run_worker();
        }
        _ => {}
    }
    // Help/version/completion parsing must not start an engine or initialize inference.
    let config = EngineConfig::for_user()?;
    if pulsar_clients::command_needs_engine(&args) {
        ensure_engine(&config)?;
    }
    pulsar_clients::run_cli(args, &config.endpoint(), &config.bootstrap_path())
}

const ENGINE_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const ENDPOINT_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

fn ensure_engine(config: &EngineConfig) -> Result<()> {
    // Every readiness probe, contender-exit check and retry shares this budget.
    let deadline = Instant::now()
        .checked_add(ENGINE_STARTUP_TIMEOUT)
        .context("engine startup timeout overflow")?;
    if endpoint_accepts_connections(config, deadline)? {
        // Authentication and version negotiation happen in the client. An existing
        // incompatible engine must never be killed or replaced by this launcher.
        return Ok(());
    }
    let executable = std::env::current_exe().context("locate Pulsar executable")?;
    let mut child = Command::new(executable)
        .arg("engine")
        .env("PULSAR_STATE_DIR", &config.state_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("start per-user Pulsar engine")?;
    loop {
        if endpoint_accepts_connections(config, deadline)? {
            // Dropping Child does not kill the engine. Authorized offline jobs
            // intentionally survive closure of their originating client.
            return Ok(());
        }
        if let Some(status) = child.try_wait().context("observe engine startup")? {
            // Another launcher can acquire the instance lock before its listener
            // exists. Wait only for actual lock contention, never mere lock-file
            // existence, and never spawn another contender in this loop.
            if endpoint_accepts_connections(config, deadline)? {
                return Ok(());
            }
            if !pulsar_engine::instance_lock_is_held(config)
                .context("inspect existing engine ownership without modifying it")?
            {
                bail!("engine startup exited with {status}; run 'pulsar engine' to see its diagnostic");
            }
        }
        std::thread::sleep(Duration::from_millis(40).min(startup_remaining(deadline)?));
    }
}

fn startup_remaining(deadline: Instant) -> Result<Duration> {
    // Do not kill a process whose successful instance ownership is unknown.
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .context("engine startup deadline exceeded; inspect the engine before retrying")
}

#[cfg(unix)]
fn endpoint_accepts_connections(config: &EngineConfig, deadline: Instant) -> Result<bool> {
    let timeout = startup_remaining(deadline)?.min(ENDPOINT_PROBE_TIMEOUT);
    match pulsar_protocol::connect_local(&config.endpoint(), timeout) {
        Ok(_) => Ok(true),
        Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => Ok(false),
        Err(error) => Err(error).context(
            "engine endpoint is busy, unresponsive, or inaccessible; existing engine left untouched"),
    }
}

#[cfg(not(unix))]
fn endpoint_accepts_connections(_config: &EngineConfig, deadline: Instant) -> Result<bool> {
    startup_remaining(deadline)?;
    // A platform-specific local transport must be supplied before this platform
    // can advertise a running engine. Never mistake a token file for readiness.
    Ok(false)
}
