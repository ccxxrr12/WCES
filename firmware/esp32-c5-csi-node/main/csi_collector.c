/**
 * @file csi_collector.c
 * @brief CSI data collection and ADR-018 binary frame serialization.
 *
 * Supports ESP32-C5 (WiFi 6), ESP32-C6, ESP32-C3, ESP32-S3, ESP32.
 * CSI callback (esp_wifi_set_csi_rx_cb) and wifi_csi_info_t are identical across chips.
 * CSI config struct differs: wifi_csi_acquire_config_t (C5/C6) vs wifi_csi_config_t (S3/C3).
 *
 * ESP32-C5 differences:
 *   - WiFi 6 (802.11ax) provides HE-LTF for higher resolution CSI.
 *   - Up to 484 subcarriers (40MHz HE) vs 114 (40MHz HT on S3).
 *   - Supports both 2.4 GHz and 5 GHz bands natively.
 *   - CSI IQ buffer in callback can be up to 4x larger than S3.
 *   - CSI performance: C5 > C6 > C3 ≈ S3 > ESP32 (per Espressif).
 *
 * ADR-029 extensions:
 *   - Channel-hop table for multi-band sensing (channels 1/6/11 by default)
 *   - Timer-driven channel hopping at configurable dwell intervals
 *   - NDP frame injection stub for sensing-first TX
 */

#include "csi_collector.h"
#include "nvs_config.h"
#include "stream_sender.h"
#include "edge_processing.h"

#include <string.h>
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"
#include "esp_log.h"
#include "esp_wifi.h"
#include "esp_timer.h"
#include "sdkconfig.h"

/* ADR-060: Access the global NVS config for MAC filter and channel override. */
extern nvs_config_t g_nvs_config;

/* ADR-057: Build-time guard — fail early if CSI is not enabled in sdkconfig.
 * Without this, the firmware compiles but crashes at runtime with:
 *   "E (xxxx) wifi:CSI not enabled in menuconfig!"
 * which is confusing for users flashing pre-built binaries. */
#ifndef CONFIG_ESP_WIFI_CSI_ENABLED
#error "CONFIG_ESP_WIFI_CSI_ENABLED must be set in sdkconfig. " \
       "Run: idf.py menuconfig -> Component config -> Wi-Fi -> Enable WiFi CSI, " \
       "or copy sdkconfig.defaults.template to sdkconfig.defaults before building."
#endif

static const char *TAG = "csi_collector";

/* KNOWN LIMITATION: s_sequence is uint32_t and wraps after ~4.3e9 frames.
 * At 50 Hz send rate this occurs after ~2.5 years of continuous operation.
 * The downstream aggregator treats sequence numbers as opaque and does not
 * depend on monotonicity, so wrap is harmless in practice. */
static uint32_t s_sequence = 0;
static uint32_t s_cb_count = 0;
static uint32_t s_send_ok = 0;
static uint32_t s_send_fail = 0;
static uint32_t s_rate_skip = 0;

/**
 * Minimum interval between UDP sends in microseconds.
 * CSI callbacks can fire hundreds of times per second in promiscuous mode.
 * We cap the send rate to avoid exhausting lwIP packet buffers (ENOMEM).
 * Default: 20 ms = 50 Hz max send rate.
 */
#define CSI_MIN_SEND_INTERVAL_US  (20 * 1000)
static int64_t s_last_send_us = 0;

/** Mutex to serialize esp_wifi_set_channel() calls from timer context.
 *  Prevents potential deadlock with the WiFi subsystem when channel-hopping
 *  fires concurrently with WiFi internal channel management. */
static SemaphoreHandle_t s_wifi_sem = NULL;

/** Ring buffer overflow drop counter — increments each time edge_enqueue_csi
 *  returns false because the SPSC ring is full. Used for diagnostics. */
static uint32_t s_ring_drops = 0;

/** Wi-Fi band detected at init time. Used to disambiguate 6 GHz channel
 *  numbers (1-233) from 2.4 GHz (1-13) since they overlap.
 *  KNOWN LIMITATION: Set once at boot and never updated. If the device
 *  roams from 2.4 GHz to 5 GHz (or vice versa), frequency derivation in
 *  csi_serialize_frame() will be wrong for the new band until reboot. */
static wifi_band_t s_wifi_band = WIFI_BAND_2G;

/* ---- ADR-029: Channel-hop state ---- */

/** Channel hop table (populated from NVS at boot or via set_hop_table). */
static uint8_t  s_hop_channels[CSI_HOP_CHANNELS_MAX] = {1, 6, 11, 36, 40, 44};

/** Number of active channels in the hop table. 1 = single-channel (no hop). */
static uint8_t  s_hop_count   = 1;

/** Dwell time per channel in milliseconds. */
static uint32_t s_dwell_ms    = 50;

/** Current index into s_hop_channels. */
static uint8_t  s_hop_index   = 0;

/** Handle for the periodic hop timer. NULL when timer is not running. */
static esp_timer_handle_t s_hop_timer = NULL;

/**
 * Serialize CSI data into ADR-018 binary frame format.
 *
 * Layout:
 *   [0..3]   Magic: 0xC5110001 (LE)
 *   [4]      Node ID
 *   [5]      Number of antennas (rx_ctrl.rx_ant + 1 if available, else 1)
 *   [6..7]   Number of subcarriers (LE u16) = len / (2 * n_antennas)
 *   [8..11]  Frequency MHz (LE u32) — derived from channel
 *   [12..15] Sequence number (LE u32)
 *   [16]     RSSI (i8)
 *   [17]     Noise floor (i8)
 *   [18..19] Reserved
 *   [20..]   I/Q data (raw bytes from ESP-IDF callback)
 */
size_t csi_serialize_frame(const wifi_csi_info_t *info, uint8_t *buf, size_t buf_len)
{
    if (info == NULL || buf == NULL || info->buf == NULL) {
        return 0;
    }

    /* BUG 11 fix: read actual antenna count from rx_ctrl instead of hardcoding.
     * C5 is single-antenna but other targets (S3) may have 2+.
     * Clamp to [1, 8] to prevent uint8_t wraparound (255+1=0 → div-by-zero). */
    uint8_t raw_ant = info->rx_ctrl.rx_ant;
    uint8_t n_antennas = (raw_ant < 8) ? (uint8_t)(raw_ant + 1) : 1;

    /* ADR-060: C5/C6/C61 may report first_word_invalid when AGC corrupts lead I/Q. */
    uint16_t iq_offset = 0;
#if CONFIG_IDF_TARGET_ESP32C5 || CONFIG_IDF_TARGET_ESP32C61 || CONFIG_IDF_TARGET_ESP32C6
    if (info->first_word_invalid && info->len > 2) {
        iq_offset = 2;  /* Skip first invalid I/Q pair. */
    }
#endif
    if (info->len <= 0) {
        return 0;
    }
    /* Bug 4: info->len is int (signed 32-bit on ESP32). The `(uint16_t)` cast
     * below is safe because: (a) the <= 0 check already eliminates negatives,
     * (b) info->len memory is 2^(len+2) capped at 2068 bytes (EDGE_MAX_IQ_BYTES)
     * which fits comfortably in uint16_t. The explicit range check defends against
     * a future where the CSI buffer grows beyond 65535 bytes. */
    if (info->len > UINT16_MAX) {
        ESP_LOGW(TAG, "CSI len %d exceeds UINT16_MAX, rejecting frame", info->len);
        return 0;
    }
    if ((uint16_t)info->len < iq_offset + 2) {
        return 0;  /* Not enough data after skipping invalid word. */
    }
    uint16_t iq_len = (uint16_t)info->len - iq_offset;
    uint16_t n_subcarriers = iq_len / (2 * n_antennas);

    size_t frame_size = CSI_HEADER_SIZE + iq_len;
    if (frame_size > buf_len) {
        ESP_LOGW(TAG, "Buffer too small: need %u, have %u", (unsigned)frame_size, (unsigned)buf_len);
        return 0;
    }

    /* Derive centre frequency from channel number and band.
     * Uses a compact descriptor table to avoid a long if-else chain.
     * Adding a new band (e.g. WiFi 7) only requires one new table row. */
    static const struct {
        wifi_band_t band;
        uint8_t    lo, hi;
        uint32_t   base_mhz;
        bool       ch_minus_one;  /* 2.4 GHz uses (ch-1)*5, others use ch*5 */
        bool       fixed_freq;    /* channel 14: use base_mhz directly, no ch*5 term */
    } BAND_TABLE[] = {
        { WIFI_BAND_2G,   1,  13, 2412, true,  false },
        { WIFI_BAND_2G,  14,  14, 2484, false, true  },  /* Japan ch14 = 2484 MHz fixed */
        { WIFI_BAND_5G,  36, 177, 5000, false, false },
        { WIFI_BAND_6G,   1, 233, 5950, false, false },  /* WiFi 6E 6 GHz — ESP32-C5 supported */
    };

    uint8_t  channel  = info->rx_ctrl.channel;
    uint32_t freq_mhz = 0;

    for (size_t i = 0; i < sizeof(BAND_TABLE) / sizeof(BAND_TABLE[0]); i++) {
        if (s_wifi_band == BAND_TABLE[i].band
            && channel >= BAND_TABLE[i].lo && channel <= BAND_TABLE[i].hi) {
            if (BAND_TABLE[i].fixed_freq) {
                freq_mhz = BAND_TABLE[i].base_mhz;
            } else {
                freq_mhz = BAND_TABLE[i].base_mhz + channel * 5;
                if (BAND_TABLE[i].ch_minus_one) freq_mhz -= 5;
            }
            break;
        }
    }

    /* Magic (LE) */
    uint32_t magic = CSI_MAGIC;
    memcpy(&buf[0], &magic, 4);

    /* Node ID (from NVS runtime config, not compile-time Kconfig) */
    buf[4] = g_nvs_config.node_id;

    /* Number of antennas */
    buf[5] = n_antennas;

    /* Number of subcarriers (LE u16) */
    memcpy(&buf[6], &n_subcarriers, 2);

    /* Frequency MHz (LE u32) */
    memcpy(&buf[8], &freq_mhz, 4);

    /* Sequence number (LE u32) */
    uint32_t seq = s_sequence++;
    memcpy(&buf[12], &seq, 4);

    /* RSSI (i8) */
    buf[16] = (uint8_t)(int8_t)info->rx_ctrl.rssi;

    /* Noise floor (i8) */
    buf[17] = (uint8_t)(int8_t)info->rx_ctrl.noise_floor;

    /* Reserved */
    buf[18] = 0;
    buf[19] = 0;

    /* I/Q data (skip invalid first word on C5/C6 if flagged) */
    memcpy(&buf[CSI_HEADER_SIZE], info->buf + iq_offset, iq_len);

    return frame_size;
}

/**
 * WiFi CSI callback — invoked by ESP-IDF when CSI data is available.
 */
static void wifi_csi_callback(void *ctx, wifi_csi_info_t *info)
{
    (void)ctx;

    /* ADR-060: MAC address filtering — drop frames from non-matching sources. */
    if (g_nvs_config.filter_mac_set) {
        if (memcmp(info->mac, g_nvs_config.filter_mac, 6) != 0) {
            return;  /* Source MAC doesn't match filter — skip frame. */
        }
    }

    s_cb_count++;

    if (s_cb_count <= 3 || (s_cb_count % 100) == 0) {
        ESP_LOGI(TAG, "CSI cb #%lu: len=%d rssi=%d ch=%d",
                 (unsigned long)s_cb_count, info->len,
                 info->rx_ctrl.rssi, info->rx_ctrl.channel);
    }

    static uint8_t frame_buf[CSI_MAX_FRAME_SIZE];
    size_t frame_len = csi_serialize_frame(info, frame_buf, sizeof(frame_buf));

    if (frame_len > 0) {
        /* Rate-limit UDP sends to avoid ENOMEM from lwIP pbuf exhaustion.
         * In promiscuous mode, CSI callbacks can fire 100-500+ times/sec.
         * We only need 20-50 Hz for the sensing pipeline. */
        int64_t now = esp_timer_get_time();
        if ((now - s_last_send_us) >= CSI_MIN_SEND_INTERVAL_US) {
            /* NOTE: sendto() may block briefly on ARP resolution.
             * Rate-limiting via CSI_MIN_SEND_INTERVAL_US mitigates this
             * but cannot eliminate the worst-case latency of ~1-2s for
             * ARP timeout. */
            int ret = stream_sender_send(frame_buf, frame_len);
            if (ret > 0) {
                s_send_ok++;
                s_last_send_us = now;
                if (s_send_ok <= 3) ESP_LOGI(TAG, "UDP OK #%lu %uB", (unsigned long)s_send_ok, (unsigned)frame_len);
            } else {
                s_send_fail++;
                if (s_send_fail <= 5) {
                    ESP_LOGW(TAG, "sendto failed (fail #%lu)", (unsigned long)s_send_fail);
                }
            }
        } else {
            s_rate_skip++;
        }
    }

    /* ADR-039: Enqueue raw I/Q into edge processing ring buffer. */
    if (info->buf && info->len > 0) {
        if (!edge_enqueue_csi((const uint8_t *)info->buf, (uint16_t)info->len,
                             (int8_t)info->rx_ctrl.rssi, info->rx_ctrl.channel)) {
            s_ring_drops++;
            if ((s_ring_drops & 0xFFF) == 0) {
                ESP_LOGW(TAG, "Ring overflow: %lu drops", (unsigned long)s_ring_drops);
            }
        }
    }
}
/* BUG 9: wifi_promiscuous_cb removed — dead code.
 * Promiscuous mode is OFF; CSI is extracted from normal STA RX path.
 * If promiscuous is re-enabled, register via esp_wifi_set_promiscuous_rx_cb(). */

void csi_collector_init(void)
{
    /* Detect the current Wi-Fi band to disambiguate channel numbers.
     * 6 GHz (ESP32-C5/C6/C61) uses channels 1-233 which overlap with
     * 2.4 GHz (1-13). This is the only reliable way to tell them apart
     * since wifi_pkt_rx_ctrl_t has no band field. */
    esp_wifi_get_band(&s_wifi_band);
    ESP_LOGI(TAG, "Wi-Fi band: %s",
             s_wifi_band == WIFI_BAND_2G ? "2.4 GHz" :
             s_wifi_band == WIFI_BAND_5G ? "5 GHz" :
             "unknown");

    /* Create mutex to serialize esp_wifi_set_channel() access from timer callback. */
    if (s_wifi_sem == NULL) {
        s_wifi_sem = xSemaphoreCreateMutex();
    }

    /* ADR-060: Determine the CSI channel.
     * Priority: 1) NVS override (--channel), 2) connected AP channel, 3) Kconfig default. */
    uint8_t csi_channel = (uint8_t)CONFIG_CSI_WIFI_CHANNEL;

    if (g_nvs_config.csi_channel > 0) {
        /* Explicit NVS override via provision.py --channel */
        csi_channel = g_nvs_config.csi_channel;
        ESP_LOGI(TAG, "Using NVS channel override: %u", (unsigned)csi_channel);
    } else {
        /* Auto-detect from connected AP */
        wifi_ap_record_t ap_info;
        if (esp_wifi_sta_get_ap_info(&ap_info) == ESP_OK && ap_info.primary > 0) {
            csi_channel = ap_info.primary;
            ESP_LOGI(TAG, "Auto-detected AP channel: %u", (unsigned)csi_channel);
        } else {
            ESP_LOGW(TAG, "Could not detect AP channel, using Kconfig default: %u",
                     (unsigned)csi_channel);
        }
    }

    /* Update the hop table's first channel to match. */
    s_hop_channels[0] = csi_channel;

    /* Enable promiscuous mode — required for reliable CSI callbacks.
     * Without this, CSI only fires on frames destined to this station,
     * which may be very infrequent on a quiet network. */
    /* Promiscuous mode disabled on C5: sniffer starves TX hardware.
     * CSI still works from normal STA RX frames (AP beacons, directed traffic).
     * Frame rate is lower (~5-15 Hz) but TX works normally. */
    ESP_LOGI(TAG, "Promiscuous mode OFF — CSI from normal STA RX, TX available");

    /* CSI configuration.
     * C5/C6/C61: wifi_csi_acquire_config_t (esp_wifi_he_types.h, ESP-IDF v5.4+).
     * S3/C3/ESP32: wifi_csi_config_t (esp_wifi_types.h, legacy API).
     * Reference: https://github.com/espressif/esp-csi/blob/master/examples/get-started/csi_recv/main/app_main.c */
#if CONFIG_IDF_TARGET_ESP32C5 || CONFIG_IDF_TARGET_ESP32C61 || \
    (CONFIG_IDF_TARGET_ESP32C6 && ESP_IDF_VERSION >= ESP_IDF_VERSION_VAL(5, 4, 0))
    /* C5/C6/C61: New CSI config API (ESP-IDF v5.4+) */
    wifi_csi_acquire_config_t csi_config = {
        .enable                   = true,
        .acquire_csi_legacy       = true,   /* L-LTF (legacy) */
        .acquire_csi_ht20         = true,   /* HT-LTF 20MHz */
        .acquire_csi_ht40         = true,   /* HT-LTF 40MHz */
        .acquire_csi_su           = true,   /* HE SU (single user) */
        .acquire_csi_mu           = true,   /* HE MU (multi user) */
        .acquire_csi_dcm          = true,   /* DCM (dual carrier modulation) */
        .acquire_csi_beamformed   = true,   /* Beamformed frames */
        .acquire_csi_force_lltf   = false,  /* false = auto, true = force L-LTF only */
        .val_scale_cfg            = 0,      /* 0 = no scaling */
        .dump_ack_en              = true,   /* Include ACK frames */
    };
#else
    /* S3/C3/ESP32: Legacy CSI config API */
    wifi_csi_config_t csi_config = {
        .lltf_en = true,           /* Legacy LTF (all chips) */
        .htltf_en = true,          /* HT LTF (802.11n/ac/ax) */
        .stbc_htltf2_en = true,    /* STBC HT-LTF stream 2 */
        .ltf_merge_en = true,      /* Merge LTF symbols for better SNR */
        .channel_filter_en = false, /* Process all subcarriers */
        .manu_scale = false,       /* No manual scaling */
        .shift = false,            /* No manual phase shift */
    };
#endif

    ESP_ERROR_CHECK(esp_wifi_set_csi_config(&csi_config));
    ESP_ERROR_CHECK(esp_wifi_set_csi_rx_cb(wifi_csi_callback, NULL));
    ESP_ERROR_CHECK(esp_wifi_set_csi(true));

    if (g_nvs_config.filter_mac_set) {
        ESP_LOGI(TAG, "MAC filter active: %02x:%02x:%02x:%02x:%02x:%02x",
                 g_nvs_config.filter_mac[0], g_nvs_config.filter_mac[1],
                 g_nvs_config.filter_mac[2], g_nvs_config.filter_mac[3],
                 g_nvs_config.filter_mac[4], g_nvs_config.filter_mac[5]);
    }

    ESP_LOGI(TAG, "CSI collection initialized (node_id=%d, channel=%u)",
             g_nvs_config.node_id, (unsigned)csi_channel);
}

/* ---- ADR-029: Channel hopping ---- */

void csi_collector_set_hop_table(const uint8_t *channels, uint8_t hop_count, uint32_t dwell_ms)
{
    if (channels == NULL) {
        ESP_LOGW(TAG, "csi_collector_set_hop_table: channels is NULL");
        return;
    }
    if (hop_count == 0 || hop_count > CSI_HOP_CHANNELS_MAX) {
        ESP_LOGW(TAG, "csi_collector_set_hop_table: invalid hop_count=%u (max=%u)",
                 (unsigned)hop_count, (unsigned)CSI_HOP_CHANNELS_MAX);
        return;
    }
    if (dwell_ms < 10) {
        ESP_LOGW(TAG, "csi_collector_set_hop_table: dwell_ms=%lu too small, clamping to 10",
                 (unsigned long)dwell_ms);
        dwell_ms = 10;
    }

    memcpy(s_hop_channels, channels, hop_count);
    s_hop_count = hop_count;
    s_dwell_ms  = dwell_ms;
    s_hop_index = 0;

    ESP_LOGI(TAG, "Hop table set: %u channels, dwell=%lu ms", (unsigned)hop_count,
             (unsigned long)dwell_ms);
    for (uint8_t i = 0; i < hop_count; i++) {
        ESP_LOGI(TAG, "  hop[%u] = channel %u", (unsigned)i, (unsigned)channels[i]);
    }
}

void csi_hop_next_channel(void)
{
    if (s_hop_count <= 1) {
        /* Single-channel mode: no-op for backward compatibility. */
        return;
    }

    s_hop_index = (s_hop_index + 1) % s_hop_count;
    uint8_t channel = s_hop_channels[s_hop_index];

    /*
     * esp_wifi_set_channel() changes the primary channel.
     * The second parameter is the secondary channel offset for HT40;
     * we use HT20 (no secondary) for sensing.
     * Guarded by a mutex to prevent race conditions with the WiFi subsystem
     * when the hop timer fires concurrently with internal channel management.
     */
    if (s_wifi_sem && xSemaphoreTake(s_wifi_sem, pdMS_TO_TICKS(100))) {
        esp_err_t err = esp_wifi_set_channel(channel, WIFI_SECOND_CHAN_NONE);
        xSemaphoreGive(s_wifi_sem);
        if (err != ESP_OK) {
            ESP_LOGW(TAG, "Channel hop to %u failed: %s", (unsigned)channel, esp_err_to_name(err));
        } else if ((s_cb_count % 200) == 0) {
            /* Periodic log to confirm hopping is working (not every hop). */
            ESP_LOGI(TAG, "Hopped to channel %u (index %u/%u)",
                     (unsigned)channel, (unsigned)s_hop_index, (unsigned)s_hop_count);
        }
    } else {
        ESP_LOGW(TAG, "Channel hop skipped: semaphore busy (WiFi subsystem may be blocked)");
    }
}

/**
 * Timer callback for channel hopping.
 * Called every s_dwell_ms milliseconds from the esp_timer context.
 */
static void hop_timer_cb(void *arg)
{
    (void)arg;
    csi_hop_next_channel();
}

void csi_collector_start_hop_timer(void)
{
    if (s_hop_count <= 1) {
        ESP_LOGI(TAG, "Single-channel mode: hop timer not started");
        return;
    }

    if (s_hop_timer != NULL) {
        ESP_LOGW(TAG, "Hop timer already running");
        return;
    }

    esp_timer_create_args_t timer_args = {
        .callback = hop_timer_cb,
        .arg      = NULL,
        .name     = "csi_hop",
    };

    esp_err_t err = esp_timer_create(&timer_args, &s_hop_timer);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "Failed to create hop timer: %s", esp_err_to_name(err));
        return;
    }

    uint64_t period_us = (uint64_t)s_dwell_ms * 1000;
    err = esp_timer_start_periodic(s_hop_timer, period_us);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "Failed to start hop timer: %s", esp_err_to_name(err));
        esp_timer_delete(s_hop_timer);
        s_hop_timer = NULL;
        return;
    }

    ESP_LOGI(TAG, "Hop timer started: period=%lu ms, channels=%u",
             (unsigned long)s_dwell_ms, (unsigned)s_hop_count);
}

/* ---- ADR-029: NDP frame injection (stub — sends minimal null-data placeholder) ----
 *
 * NOTE: This sends a hardcoded 24-byte null-data frame, NOT a true 802.11 NDP
 * (which would be preamble-only, ~24 us airtime, no MAC payload).
 *
 * For competition demo purposes the stub is sufficient; the API is wired up so
 * a proper NDP can be substituted after the competition without changing callers.
 *
 * To implement a real NDP: use esp_wifi_80211_tx() with WIFI_PKT_MGMT and a
 * properly constructed preamble-only frame per IEEE 802.11ax 26.5.2. */

esp_err_t csi_inject_ndp_frame(void)
{
    uint8_t ndp_frame[24];
    memset(ndp_frame, 0, sizeof(ndp_frame));

    /* Frame Control: Type=Data (0x02), Subtype=Null (0x04) -> 0x0048 */
    ndp_frame[0] = 0x48;
    ndp_frame[1] = 0x00;

    /* Duration: 0 (let hardware fill) */

    /* Addr1 (destination): broadcast */
    memset(&ndp_frame[4], 0xFF, 6);

    /* Addr2 (source): will be overwritten by hardware with own MAC */

    /* Addr3 (BSSID): broadcast */
    memset(&ndp_frame[16], 0xFF, 6);

    esp_err_t err = esp_wifi_80211_tx(WIFI_IF_STA, ndp_frame, sizeof(ndp_frame), false);
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "NDP inject failed: %s", esp_err_to_name(err));
    }

    return err;
}
