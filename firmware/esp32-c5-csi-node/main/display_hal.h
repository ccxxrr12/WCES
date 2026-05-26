/**
 * @file display_hal.h
 * @brief ADR-045: SH8601 QSPI AMOLED + FT3168 touch HAL.
 *
 * Hardware abstraction for the Waveshare ESP32-S3-Touch-AMOLED-1.8 panel.
 * Uses TCA9554 I/O expander for display power/reset control.
 * Probes hardware at boot; returns ESP_ERR_NOT_FOUND if absent.
 *
 * NOTE: This HAL targets ESP32-S3 Waveshare boards. For ESP32-C5, pin
 * assignments must be verified against the actual wiring.
 */

#ifndef DISPLAY_HAL_H
#define DISPLAY_HAL_H

#include <stdbool.h>
#include <stdint.h>
#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Probe and initialize the SH8601 QSPI AMOLED panel.
 *
 * Initializes I2C bus, configures TCA9554 I/O expander for power/reset,
 * sets up QSPI bus, sends SH8601 init sequence, and draws a test pattern.
 * Returns ESP_ERR_NOT_FOUND if the panel does not respond.
 *
 * @return ESP_OK on success, ESP_ERR_NOT_FOUND if no display detected.
 */
esp_err_t display_hal_init_panel(void);

/**
 * Draw a rectangle of pixels to the AMOLED.
 * Sends CASET + RASET + RAMWR directly via QSPI.
 *
 * @param x_start  Left column (inclusive).
 * @param y_start  Top row (inclusive).
 * @param x_end    Right column (exclusive).
 * @param y_end    Bottom row (exclusive).
 * @param color_data  RGB565 pixel data, (x_end-x_start)*(y_end-y_start) pixels.
 */
void display_hal_draw(int x_start, int y_start, int x_end, int y_end,
                      const void *color_data);

/**
 * Probe and initialize the FT3168 capacitive touch controller.
 *
 * @return ESP_OK on success, ESP_ERR_NOT_FOUND if no touch IC detected.
 */
esp_err_t display_hal_init_touch(void);

/**
 * Read touch point (non-blocking).
 *
 * @param[out] x  Touch X coordinate (0..367).
 * @param[out] y  Touch Y coordinate (0..447).
 * @return true if touch is active, false if released.
 */
bool display_hal_touch_read(uint16_t *x, uint16_t *y);

/**
 * Set AMOLED brightness via MIPI DCS command.
 *
 * @param percent  Brightness 0-100.
 */
void display_hal_set_brightness(uint8_t percent);

#ifdef __cplusplus
}
#endif

#endif /* DISPLAY_HAL_H */
