//! Broadcast tick task: periodically re-broadcasts the latest sensing update
//! over the broadcast channel (for ESP32 mode, when frames may pause between packets).

use std::time::Duration;

use crate::SharedState;

pub(crate) async fn broadcast_tick_task(state: SharedState, tick_ms: u64) {
    let mut interval = tokio::time::interval(Duration::from_millis(tick_ms));

    loop {
        interval.tick().await;

        // Drain and broadcast pending alerts from AlertingBridge
        // (dead data flow fix #4: wire alerting_bridge → WebSocket).
        {
            let mut s = state.write().await;
            let alerts = s.alerting_bridge.drain_alerts();
            for alert in &alerts {
                if let Ok(json) = serde_json::to_string(&serde_json::json!({
                    "type": "alert",
                    "id": alert.id,
                    "survivor_id": alert.survivor_id,
                    "title": alert.title,
                    "message": alert.message,
                    "priority": alert.priority,
                    "status": alert.status,
                    "triage_status": alert.triage_status,
                    "created_at_secs": alert.created_at_secs,
                })) {
                    let _ = s.tx.send(json);
                }
            }
        }

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
