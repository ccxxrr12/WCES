/**
 * @file csi_collector.h
 * @brief CSI data collection and ADR-018 binary frame serialization.
 *
 * Supports ESP32-C5 (WiFi 6), ESP32-C6, ESP32-C3, ESP32-S3, ESP32.
 * ESP32-C5 provides the best CSI quality: C5 > C6 > C3 ≈ S3 > ESP32.
 */

#ifndef CSI_COLLECTOR_H
#define CSI_COLLECTOR_H

#include <stdint.h>
#include <stddef.h>
#include "esp_err.h"
#include "esp_wifi_types.h"

/** ADR-018 magic number. */
#define CSI_MAGIC 0xC5110001

/** ADR-018 header size in bytes. */
#define CSI_HEADER_SIZE 20

/** Maximum frame buffer size.
 *  WiFi 6 (C5/C6) 40MHz: up to 484 subcarriers per antenna.
 *  WiFi 4 (S3/C3) 40MHz: up to 114 subcarriers.
 *  Formula: header + max_antennas * max_subcarriers * 2 bytes (I/Q)
 *  = 20 + 4 * 512 * 2 = 4116 bytes.
 *  NOTE: Stack-allocated in CSI callback. C5 has 400KB SRAM; ensure
 *  CONFIG_ESP_MAIN_TASK_STACK_SIZE >= 7168 to accommodate this. */
#define CSI_MAX_FRAME_SIZE (CSI_HEADER_SIZE + 4 * 512 * 2)

/** Maximum number of channels in the hop table (ADR-029). */
#define CSI_HOP_CHANNELS_MAX 6

/**
 * Initialize CSI collection.
 * Registers the WiFi CSI callback.
 */
void csi_collector_init(void);

/**
 * Serialize CSI data into ADR-018 binary frame format.
 *
 * @param info   WiFi CSI info from the ESP-IDF callback.
 * @param buf    Output buffer (must be at least CSI_MAX_FRAME_SIZE bytes).
 * @param buf_len Size of the output buffer.
 * @return Number of bytes written, or 0 on error.
 */
size_t csi_serialize_frame(const wifi_csi_info_t *info, uint8_t *buf, size_t buf_len);

/**
 * Configure the channel-hop table for multi-band sensing (ADR-029).
 *
 * When hop_count == 1 the collector stays on the single configured channel
 * (backward-compatible with the original single-channel mode).
 *
 * @param channels  Array of WiFi channel numbers (1-14 for 2.4 GHz, 36-177 for 5 GHz).
 * @param hop_count Number of entries in the channels array (1..CSI_HOP_CHANNELS_MAX).
 * @param dwell_ms  Dwell time per channel in milliseconds (>= 10).
 */
void csi_collector_set_hop_table(const uint8_t *channels, uint8_t hop_count, uint32_t dwell_ms);

/**
 * Advance to the next channel in the hop table.
 *
 * Called by the hop timer callback. If hop_count <= 1 this is a no-op.
 * Calls esp_wifi_set_channel() internally.
 */
void csi_hop_next_channel(void);

/**
 * Start the channel-hop timer.
 *
 * Creates an esp_timer periodic callback that fires every dwell_ms
 * milliseconds, calling csi_hop_next_channel(). If hop_count <= 1
 * the timer is not started (single-channel backward-compatible mode).
 */
void csi_collector_start_hop_timer(void);

/**
 * Inject an NDP (Null Data Packet) frame for sensing.
 *
 * Uses esp_wifi_80211_tx() to send a preamble-only frame (~24 us airtime)
 * that triggers CSI measurement at all receivers. This is the "sensing-first"
 * TX mechanism described in ADR-029.
 *
 * @return ESP_OK on success, or an error code.
 *
 * @note TODO: Full NDP frame construction. Currently sends a minimal
 *       null-data frame as a placeholder.
 */
esp_err_t csi_inject_ndp_frame(void);

#endif /* CSI_COLLECTOR_H */
