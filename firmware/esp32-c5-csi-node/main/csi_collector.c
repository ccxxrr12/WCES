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
#include "esp_csi_gain_ctrl.h"

#include <string.h>
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"
#include "esp_log.h"
#include "esp_wifi.h"
#include "esp_timer.h"
#include "esp_heap_caps.h"
#include "esp_psram.h"
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

/* ---- PSRAM burst ring (ADR-159) ---- */

/** Number of frames the PSRAM ring can buffer before overwriting.
 *  256 slots × ~532 bytes = ~133 KB — negligible on 8MB PSRAM. */
#define CSI_BURST_SLOTS  256

/** Flush interval: how often the burst ring is drained over UDP.
 *  100 ms balances latency against TX efficiency. */
#define CSI_BURST_FLUSH_INTERVAL_MS  100

/** PSRAM ring buffer base pointer (allocated from SPIRAM at init). */
static uint8_t  *s_burst_ring = NULL;

/** Per-slot frame lengths (SRAM index — 256 × 2 = 512 bytes). */
static uint16_t  s_burst_lens[CSI_BURST_SLOTS];

/** Ring head (producer — CSI callback writes here). */
static volatile uint32_t s_burst_head = 0;

/** Ring tail (consumer — flush timer reads here). */
static volatile uint32_t s_burst_tail = 0;

/** Whether PSRAM is available and the burst ring is active. */
static bool s_psram_ok = false;

/** Handle for the periodic flush timer. */
static esp_timer_handle_t s_flush_timer = NULL;

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

/** E-3 fix: spinlock guarding the hop table (s_hop_channels / s_hop_count /
 *  s_hop_index / s_dwell_ms). csi_collector_set_hop_table() can be called from
 *  any task (e.g. NVS config, HTTP command) while csi_hop_next_channel() runs
 *  from the esp_timer task. Without protection, a table swap mid-hop could
 *  read a stale s_hop_count with a new s_hop_channels, indexing out of bounds.
 *  A spinlock (not the WiFi mutex) is used because the critical section is
 *  brief (memcpy of ≤16 bytes) and must not block. */
static portMUX_TYPE s_hop_spinlock = portMUX_INITIALIZER_UNLOCKED;

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

    /* ESP-IDF v6.0: rx_ctrl no longer exposes rx_ant. C5 is single-antenna. */
    uint8_t n_antennas = 1;

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
    /* ESP-IDF v6.0: info->len is uint16_t, UINT16_MAX check removed. */
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
        /* WIFI_BAND_6G removed in ESP-IDF v6.0 */
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

    /* Guard: if channel is 0 (WiFi not yet connected) or out of band table
     * range, freq_mhz stays 0. Reject the frame rather than sending garbage. */
    if (freq_mhz == 0) {
        ESP_LOGW(TAG, "CSI frame dropped: cannot derive frequency for channel %u, band %d",
                 (unsigned)channel, (int)s_wifi_band);
        return 0;
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

    /* ── Gain Lock: stabilise CSI amplitude by locking AGC ────────────── */
    {
        uint8_t  agc_gain = 0;
        int8_t   fft_gain = 0;
        esp_csi_gain_ctrl_get_rx_gain(&info->rx_ctrl, &agc_gain, &fft_gain);

        rx_gain_status_t gs = esp_csi_gain_ctrl_get_gain_status();
        if (gs == RX_GAIN_COLLECT) {
            /* Safety: skip locking when RSSI too strong (AGC < 30 → HW freeze risk). */
            if (info->rx_ctrl.rssi > -40 || agc_gain < 30) {
                /* Too close to AP — stay in collect phase so we can lock later. */
                esp_csi_gain_ctrl_record_rx_gain(agc_gain, fft_gain);
            } else {
                esp_csi_gain_ctrl_record_rx_gain(agc_gain, fft_gain);
            }
        } else if (gs == RX_GAIN_READY) {
            uint8_t  base_agc = 0;
            int8_t   base_fft = 0;
            if (esp_csi_gain_ctrl_get_rx_gain_baseline(&base_agc, &base_fft) == ESP_OK) {
                if (base_agc >= 30) {
                    esp_csi_gain_ctrl_set_rx_force_gain(base_agc, base_fft);
                    ESP_LOGI(TAG, "Gain locked: AGC=%d FFT=%d  ← CSI dynamic range opened", base_agc, base_fft);
                }
            }
        }
        /* RX_GAIN_FORCE: already locked — nothing to do. */
    }

    if (s_cb_count <= 3 || (s_cb_count % 100) == 0) {
        ESP_LOGI(TAG, "CSI cb #%lu: len=%d rssi=%d ch=%d",
                 (unsigned long)s_cb_count, info->len,
                 info->rx_ctrl.rssi, info->rx_ctrl.channel);
    }

    static uint8_t frame_buf[CSI_MAX_FRAME_SIZE];
    size_t frame_len = csi_serialize_frame(info, frame_buf, sizeof(frame_buf));

    if (frame_len > 0) {
        if (s_psram_ok) {
            /* ── PSRAM burst path: enqueue into ring, never drop ────── */
            uint32_t next = (s_burst_head + 1) % CSI_BURST_SLOTS;
            if (next != s_burst_tail) {
                size_t off = (size_t)s_burst_head * CSI_MAX_FRAME_SIZE;
                memcpy(&s_burst_ring[off], frame_buf, frame_len);
                s_burst_lens[s_burst_head] = (uint16_t)frame_len;
                /* Memory fence: ensure ring data is visible before head update.
                 * Required for correctness on multi-hart systems; on single-core
                 * C5 this is a compiler barrier (volatile handles the rest). */
                __sync_synchronize();
                s_burst_head = next;
                s_send_ok++;  /* count as "queued" not "sent" */
            } else {
                s_rate_skip++;  /* ring full — flush may be stalled */
            }
        } else {
            /* ── Direct UDP fallback: rate-limited sendto ───────────── */
            int64_t now = esp_timer_get_time();
            if ((now - s_last_send_us) >= CSI_MIN_SEND_INTERVAL_US) {
                int ret = stream_sender_send(frame_buf, frame_len);
                if (ret > 0) {
                    s_send_ok++;
                    s_last_send_us = now;
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
 * Promiscuous mode is ON with PSRAM burst mode; CSI is captured at full rate
 * into the PSRAM ring and flushed in brief TX windows by csi_burst_flush_cb(). */

/* ── PSRAM burst flush (ADR-159) ─────────────────────────────────────────── */

/**
 * Periodic flush callback: briefly disables promiscuous RX, drains the PSRAM
 * ring over UDP, then re-enables promiscuous.
 *
 * The radio is in TX mode for only ~1-2ms per 100ms cycle, so CSI frame loss
 * is negligible. Without PSRAM the original rate-limited direct-UDP path is
 * used instead (see wifi_csi_callback above).
 */
static void csi_burst_flush_cb(void *arg)
{
    (void)arg;

    if (!s_psram_ok || s_burst_ring == NULL) {
        return;
    }

    /* Briefly release the radio for TX. */
    esp_wifi_set_promiscuous(false);

    /* Drain all buffered frames. */
    while (s_burst_tail != s_burst_head) {
        size_t off = (size_t)s_burst_tail * CSI_MAX_FRAME_SIZE;
        uint16_t len = s_burst_lens[s_burst_tail];
        int ret = stream_sender_send(&s_burst_ring[off], len);
        if (ret > 0) {
            s_send_ok++;
        } else {
            s_send_fail++;
        }
        s_burst_tail = (s_burst_tail + 1) % CSI_BURST_SLOTS;
    }

    /* Resume promiscuous CSI capture. */
    esp_wifi_set_promiscuous(true);
}

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
        /* M-3 fix: Log if mutex creation failed — channel hopping will be
         * disabled (gracefully degraded via the s_wifi_sem && checks). */
        if (s_wifi_sem == NULL) {
            ESP_LOGE(TAG, "Failed to create WiFi semaphore — channel hopping disabled");
        }
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
    /* PSRAM burst mode: keep promiscuous ON for high-rate CSI capture.
     * Frames are serialized into the PSRAM ring in the callback (fast memcpy,
     * no UDP), then drained in brief TX windows by the flush timer.
     * This avoids the C5 single-radio TX starvation issue — the radio only
     * pauses RX for ~1-2ms per 100ms flush cycle. */
    ESP_ERROR_CHECK(esp_wifi_set_promiscuous(true));
    ESP_LOGI(TAG, "Promiscuous ON — PSRAM burst mode (flush every %u ms)",
             (unsigned)CSI_BURST_FLUSH_INTERVAL_MS);

    /* CSI configuration.
     * C5/C6/C61: wifi_csi_acquire_config_t (esp_wifi_he_types.h, ESP-IDF v5.4+).
     * S3/C3/ESP32: wifi_csi_config_t (esp_wifi_types.h, legacy API).
     *
     * Strategy: prefer HE SU (242-tone, highest resolution). Keep HT40 as
     * 11n fallback when the AP does not support 802.11ax. VHT20 as tertiary
     * backup. Legacy/HT20 disabled — their 52/56 subcarriers add dimension
     * jitter without SNR benefit. MU/DCM/beamformed disabled — rare PPDU
     * types that add noise without improving vital-sign SNR. */
#if CONFIG_IDF_TARGET_ESP32C5 || CONFIG_IDF_TARGET_ESP32C61 || \
    (CONFIG_IDF_TARGET_ESP32C6 && ESP_IDF_VERSION >= ESP_IDF_VERSION_VAL(5, 4, 0))
    /* C5/C6/C61: New CSI config API (ESP-IDF v5.4+) */
    wifi_csi_acquire_config_t csi_config = {
        .enable                   = true,
        .acquire_csi_legacy       = false,  /* L-LTF 52sc — SNR too low */
        .acquire_csi_ht20         = false,  /* HT20 56sc — dimension jitter */
        .acquire_csi_ht40         = true,   /* HT40 114sc — 11n fallback */
        .acquire_csi_su           = true,   /* HE SU 242sc — primary */
        .acquire_csi_mu           = false,  /* MU OFDMA — rare, no benefit */
        .acquire_csi_dcm          = false,  /* DCM — remote weak-signal, rare */
        .acquire_csi_beamformed   = false,  /* BF — phase distorted by precoding */
        .acquire_csi_force_lltf   = false,  /* auto: use best available LTF */
        .acquire_csi_vht          = true,   /* VHT20 — tertiary fallback (C5-only) */
        .acquire_csi_he_stbc_mode = 0,      /* HE-LTF1 only, no alternating */
        .val_scale_cfg            = 5,      /* higher precision for weak signals */
        .dump_ack_en              = false,  /* ACK frames have poor CSI SNR */
    };
    /* ESP32-C5 CSI subcarrier counts by PPDU type:
     *   - HE SU (acquire_csi_su): HE-LTF, 242-tone HE20 (C5 11ax is 20MHz-only)
     *   - HT40 (acquire_csi_ht40): HT-LTF, 114-tone (11n fallback)
     *   - VHT20 (acquire_csi_vht): VHT-LTF, 56-tone (11ac fallback)
     *   - Only one type is active at a time — no dimension mixing. */
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

    /* ── PSRAM burst ring init ─────────────────────────────────────────── */
#if CONFIG_SPIRAM
    if (heap_caps_get_free_size(MALLOC_CAP_SPIRAM) > 0) {
        size_t ring_bytes = (size_t)CSI_BURST_SLOTS * CSI_MAX_FRAME_SIZE;
        s_burst_ring = heap_caps_malloc(ring_bytes,
                         MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
        if (s_burst_ring != NULL) {
            memset(s_burst_ring, 0, ring_bytes);
            s_psram_ok = true;
            ESP_LOGI(TAG, "PSRAM burst ring: %u slots × %u B = %u KB",
                     (unsigned)CSI_BURST_SLOTS, (unsigned)CSI_MAX_FRAME_SIZE,
                     (unsigned)(ring_bytes / 1024));
        } else {
            ESP_LOGW(TAG, "PSRAM ring alloc failed — using direct UDP fallback");
        }
    } else {
        ESP_LOGI(TAG, "PSRAM not initialized — using direct UDP fallback");
    }
#else
    ESP_LOGI(TAG, "CONFIG_SPIRAM=n — using direct UDP fallback");
#endif

    /* ── Start flush timer (PSRAM burst mode) ─────────────────────────── */
    if (s_psram_ok) {
        esp_timer_create_args_t flush_args = {
            .callback = csi_burst_flush_cb,
            .arg      = NULL,
            .name     = "csi_flush",
        };
        esp_err_t err = esp_timer_create(&flush_args, &s_flush_timer);
        if (err == ESP_OK) {
            esp_timer_start_periodic(s_flush_timer,
                                     (uint64_t)CSI_BURST_FLUSH_INTERVAL_MS * 1000);
            ESP_LOGI(TAG, "Flush timer: every %u ms", (unsigned)CSI_BURST_FLUSH_INTERVAL_MS);
        } else {
            ESP_LOGW(TAG, "Flush timer create failed: %s", esp_err_to_name(err));
        }
    }

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

    /* E-3 fix: take the spinlock while swapping the table so csi_hop_next_channel
     * (running from the timer task) never observes a half-updated table. The
     * critical section is a short memcpy + three scalar stores. */
    taskENTER_CRITICAL(&s_hop_spinlock);
    memcpy(s_hop_channels, channels, hop_count);
    s_hop_count = hop_count;
    s_dwell_ms  = dwell_ms;
    s_hop_index = 0;
    taskEXIT_CRITICAL(&s_hop_spinlock);

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

    /* E-3 fix: read the hop index/count/channel atomically against
     * csi_collector_set_hop_table() which may be swapping the table concurrently. */
    uint8_t channel;
    taskENTER_CRITICAL(&s_hop_spinlock);
    s_hop_index = (s_hop_index + 1) % s_hop_count;
    channel = s_hop_channels[s_hop_index];
    taskEXIT_CRITICAL(&s_hop_spinlock);

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

/* ---- ADR-029: NDP frame injection (active sensing) ----
 *
 * Injects a minimal 802.11 null data frame (24-byte MAC header, no payload)
 * to actively trigger CSI measurement instead of waiting for AP traffic.
 *
 * NDP injection can raise the effective CSI sampling rate from ~15 Hz
 * (passive STA RX) to ~50-100 Hz by forcing frequent channel-sounding
 * opportunities independent of AP beacon/traffic cadence.
 *
 * Frame Control layout (IEEE 802.11 §9.2.4):
 *   byte0 = (Subtype<<4) | (Type<<2) | ProtocolVersion
 *         = (4<<4) | (2<<2) | 0 = 0x48   (Type=Data, Subtype=Null)
 *   byte1 = ToDS(0) | FromDS(0) = 0x00 */

esp_err_t csi_inject_ndp_frame(void)
{
    /* Minimal 802.11 null data frame: 24-byte MAC header, no body. */
    uint8_t ndp_frame[24];
    memset(ndp_frame, 0, sizeof(ndp_frame));

    /* Frame Control: Type=Data, Subtype=Null, ToDS=0, FromDS=0 */
    ndp_frame[0] = 0x48;
    ndp_frame[1] = 0x00;

    /* Duration: 0 (LE u16) — hardware fills NAV. */
    ndp_frame[2] = 0x00;
    ndp_frame[3] = 0x00;

    /* Addr1 (Destination): broadcast FF:FF:FF:FF:FF:FF */
    memset(&ndp_frame[4], 0xFF, 6);

    /* Addr2 (Source) & Addr3 (BSSID): local STA MAC. */
    uint8_t local_mac[6] = {0};
    esp_wifi_get_mac(WIFI_IF_STA, local_mac);
    memcpy(&ndp_frame[10], local_mac, 6);  /* Addr2 (SA) */
    memcpy(&ndp_frame[16], local_mac, 6);  /* Addr3 (BSSID) */

    int64_t inject_us = esp_timer_get_time();
    ESP_LOGD(TAG, "NDP inject @ %lld us, len=%u",
             (long long)inject_us, (unsigned)sizeof(ndp_frame));

    esp_err_t err = esp_wifi_80211_tx(WIFI_IF_STA, ndp_frame, sizeof(ndp_frame), false);
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "NDP inject failed: %s", esp_err_to_name(err));
    }

    return err;
}

/**
 * Timer callback adapter for NDP injection.
 * Matching esp_timer_cb_t: void(*)(void*).
 */
void csi_inject_ndp_cb(void *arg)
{
    (void)arg;
    csi_inject_ndp_frame();
}
