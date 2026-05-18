//! Broadcast tick task: periodically re-broadcasts the latest sensing update
//! over the broadcast channel (for ESP32 mode, when frames may pause between packets).

use std::time::Duration;

use crate::SharedState;

pub(crate) async fn broadcast_tick_task(state: SharedState, tick_ms: u64) {
    let mut interval = tokio::time::interval(Duration::from_millis(tick_ms));

    loop {
        interval.tick().await;
        let s = state.read().await;
        if let Some(ref update) = s.latest_update {
            if s.tx.receiver_count() > 0 {
                // Re-broadcast the latest sensing_update so pose WS clients
                // always get data even when ESP32 pauses between frames.
                if let Ok(json) = serde_json::to_string(update) {
                    let _ = s.tx.send(json);
                }
            }
        }
    }
}
