/**
 * @file power_mgmt.c
 * @brief Power management for battery-powered ESP32-C5 CSI nodes.
 *
 * Uses ESP-IDF's automatic light sleep with WiFi modem sleep (WIFI_PS_MIN_MODEM).
 * The duty_cycle_pct parameter is logged but the actual duty cycling is delegated
 * to ESP-IDF's tickless idle / automatic light sleep — there is no manual timer-based
 * duty cycle loop. Statistics (s_active_ms, s_sleep_ms, s_wake_count) track runtime
 * but are not actively updated without CONFIG_PM_ENABLE + light sleep active.
 *
 * For competition demo: power saving is secondary to CSI data quality.
 * Set duty_cycle_pct=100 (or omit the call) to keep the radio always-on.
 */

#include "power_mgmt.h"

#include "esp_log.h"
#include "esp_pm.h"
#include "esp_wifi.h"
#include "esp_sleep.h"
#include "esp_timer.h"

static const char *TAG = "power_mgmt";

/* E-5 TODO: these statistics are currently never updated after init.
 * s_active_ms / s_sleep_ms / s_wake_count are meant to track time spent in
 * active vs. light-sleep states and the number of wakeups, but without a PM
 * hook registered (or instrumentation in the light-sleep state machine) they
 * remain at their initial values and power_mgmt_stats() always reports zeros.
 *
 * To wire them up:
 *   1. Enable CONFIG_PM_ENABLE in menuconfig.
 *   2. Register a hook via esp_pm_register_hook() (ESP-IDF v5.x) for
 *      PM_EVENT_MAX_FREQ / PM_EVENT_MIN_FREQ / PM_EVENT_LIGHT_SLEEP that
 *      timestamps transitions with esp_timer_get_time() and accumulates into
 *      s_active_ms / s_sleep_ms, incrementing s_wake_count on each wake.
 *   3. Alternatively, call these accumulators from the light-sleep entry/exit
 *      path if a manual duty-cycle loop is added later.
 * Until then, the values below are placeholders so power_mgmt_stats() has a
 * defined (if uninformative) contract. */
static uint32_t s_active_ms  = 0;
static uint32_t s_sleep_ms   = 0;
static uint32_t s_wake_count = 0;
static int64_t  s_last_wake  = 0;

esp_err_t power_mgmt_init(uint8_t duty_cycle_pct)
{
    if (duty_cycle_pct >= 100) {
        ESP_LOGI(TAG, "Power management disabled (duty_cycle=100%%)");
        return ESP_OK;
    }

    if (duty_cycle_pct < 10) {
        duty_cycle_pct = 10;
        ESP_LOGW(TAG, "Duty cycle clamped to 10%% minimum");
    }

    ESP_LOGI(TAG, "Initializing power management (duty_cycle=%u%%)", duty_cycle_pct);

    /* Enable WiFi power save mode (modem sleep). */
    esp_err_t err = esp_wifi_set_ps(WIFI_PS_MIN_MODEM);
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "WiFi power save failed: %s (continuing without PM)",
                 esp_err_to_name(err));
        return err;
    }

    /* Configure automatic light sleep via power management.
     * ESP-IDF will enter light sleep when no tasks are ready to run. */
#if CONFIG_PM_ENABLE
    esp_pm_config_t pm_config = {
        .max_freq_mhz = 240,
        .min_freq_mhz = 80,
        .light_sleep_enable = true,
    };

    err = esp_pm_configure(&pm_config);
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "PM configure failed: %s", esp_err_to_name(err));
        return err;
    }

    ESP_LOGI(TAG, "Light sleep enabled: max=%dMHz, min=%dMHz",
             pm_config.max_freq_mhz, pm_config.min_freq_mhz);
#else
    ESP_LOGW(TAG, "CONFIG_PM_ENABLE not set — light sleep unavailable. "
             "Enable in menuconfig: Component config → Power Management");
#endif

    s_last_wake = esp_timer_get_time();
    s_wake_count = 1;

    ESP_LOGI(TAG, "Power management initialized (WiFi modem sleep active)");
    return ESP_OK;
}

void power_mgmt_stats(uint32_t *active_ms, uint32_t *sleep_ms, uint32_t *wake_count)
{
    if (active_ms)  *active_ms  = s_active_ms;
    if (sleep_ms)   *sleep_ms   = s_sleep_ms;
    if (wake_count) *wake_count = s_wake_count;
}
