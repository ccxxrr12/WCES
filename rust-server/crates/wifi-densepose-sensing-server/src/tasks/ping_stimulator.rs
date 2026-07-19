//! ESP32 CSI ping stimulator — keeps WiFi data frames flowing so CSI
//! callbacks fire on the ESP32-C5 nodes.
//!
//! The ESP32-C5 CSI callback (`wifi_csi_callback`) fires only when the
//! hardware receives a **data frame** (beacons and management frames are
//! invisible to CSI). On a dedicated competition router with no other
//! clients, the three ESP32 nodes sit idle generating no CSI events.
//!
//! This module pings each node in a steady loop, forcing the AP to
//! deliver ICMP echo requests → ESP32 processes the frame → CSI callback
//! fires → UDP stream flows to the RZ/G2L server.

use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{debug, info, warn};

/// IP addresses for the three ESP32-C5 nodes (fixed in competition deployment).
const ESP32_IPS: [&str; 3] = ["192.168.1.4", "192.168.1.5", "192.168.1.6"];

/// Interval between pings per node (ms).  100 ms → 10 Hz per node →
/// 30 Hz aggregate across three nodes.  Enough for stable CSI without
/// saturating the 100 Hz flush timer.
const PING_INTERVAL_MS: u64 = 100;

/// Hard deadline for the entire ping subprocess (seconds).  BusyBox
/// and iputils both honour `-w` (deadline).  If the ping doesn't
/// complete before the deadline the kernel kills the process group.
const PING_DEADLINE_SECS: u64 = 2;

/// Launch one `ping -c 1 -w <deadline> <ip>` subprocess.
///
/// **BusyBox / iputils compatibility**
/// - `-c 1`       — send exactly one ICMP echo request (both).
/// - `-w DEADLINE`— kill the whole process after DEADLINE seconds (both).
///
/// Note: `-W` (per-packet timeout in seconds, iputils-only) is **not**
/// used because BusyBox interprets `-W` as a **deadline** with different
/// semantics and some builds reject it outright.  `-w` is the portable
/// deadline flag.
///
/// The child is spawned with `create_process_group` and we use
/// `tokio::time::timeout` as a second safety net.
async fn ping_once(ip: &str) {
    let child = Command::new("ping")
        .arg("-c")
        .arg("1")
        .arg("-w")
        .arg(PING_DEADLINE_SECS.to_string())
        .arg(ip)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            warn!("ping_stimulator: spawn ping {}: {}", ip, e);
            return;
        }
    };

    // Second safety net: kernel deadline (`-w`) should be enough, but
    // if the OS ping binary ignores it we still don't want to block
    // forever.  2× deadline + 1 s of slack.
    let wait_limit = Duration::from_secs(PING_DEADLINE_SECS * 2 + 1);
    let result = tokio::time::timeout(wait_limit, child.wait()).await;

    match result {
        Ok(Ok(status)) => {
            if status.success() {
                debug!("ping_stimulator: {} OK", ip);
            }
            // Non-zero exit = no reply (node off-line); not an error.
        }
        Ok(Err(e)) => warn!("ping_stimulator: {} wait error: {}", ip, e),
        Err(_elapsed) => {
            // .kill_on_drop(true) already SIGKILLs the child when `child`
            // is dropped at the end of this scope; we just log it.
            warn!("ping_stimulator: {} timed out after {:?}", ip, wait_limit);
        }
    }
}

/// Background task: ping all three ESP32 nodes in a round-robin loop.
///
/// Each iteration pings one node and then sleeps `PING_INTERVAL_MS`,
/// giving ~10 Hz CSI stimulation per node.  The task never exits unless
/// the runtime is torn down.
pub(crate) async fn ping_stimulator_task() {
    info!(
        ips = ?ESP32_IPS,
        interval_ms = PING_INTERVAL_MS,
        deadline_secs = PING_DEADLINE_SECS,
        "ping_stimulator: starting — will keep ESP32 CSI callbacks firing"
    );

    let mut idx: usize = 0;
    loop {
        ping_once(ESP32_IPS[idx]).await;
        idx = (idx + 1) % ESP32_IPS.len();
        sleep(Duration::from_millis(PING_INTERVAL_MS)).await;
    }
}
