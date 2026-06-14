/**
 * @file psk_auth.h
 * @brief Shared PSK authentication helpers (ADR-050).
 *
 * Used by both ota_update.c and wasm_upload.c to avoid duplicating
 * the constant-time comparison and Bearer token extraction logic.
 * Also centralises the NVS namespace/key defines so both modules
 * stay in sync when the PSK location changes.
 */

#pragma once

#include <stdbool.h>
#include <string.h>
#include <stdint.h>
#include "esp_http_server.h"

/** NVS namespace and key for the shared PSK (OTA + WASM). */
#define PSK_AUTH_NVS_NAMESPACE "security"
#define PSK_AUTH_NVS_KEY       "ota_psk"
#define PSK_AUTH_MAX_LEN       65

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Constant-time byte comparison — prevents timing side-channels.
 *
 * Iterates max(a_len, b_len) bytes. XOR accumulates differences;
 * length mismatch is folded into the accumulator so the loop count
 * does not leak the position of the first differing byte.
 *
 * @return true if equal, false otherwise (in constant time).
 */
static inline bool psk_constant_time_eq(const uint8_t *a, size_t a_len,
                                        const uint8_t *b, size_t b_len)
{
    size_t max_len = (a_len > b_len) ? a_len : b_len;
    volatile uint8_t result = (uint8_t)(a_len != b_len);
    for (size_t i = 0; i < max_len; i++) {
        uint8_t a_byte = (i < a_len) ? a[i] : 0;
        uint8_t b_byte = (i < b_len) ? b[i] : 0;
        result |= (a_byte ^ b_byte);
    }
    return result == 0;
}

/**
 * Extract and verify the Bearer token from the Authorization header.
 *
 * @return true if the token matches @p psk (constant-time), false otherwise.
 */
static inline bool psk_verify_bearer(httpd_req_t *req, const char *psk)
{
    char auth_header[128] = {0};
    if (httpd_req_get_hdr_value_str(req, "Authorization", auth_header,
                                     sizeof(auth_header)) != ESP_OK) {
        return false;
    }

    const char *prefix = "Bearer ";
    size_t prefix_len = strlen(prefix);
    if (strncmp(auth_header, prefix, prefix_len) != 0) {
        return false;
    }

    const char *token = auth_header + prefix_len;
    return psk_constant_time_eq((const uint8_t *)psk, strlen(psk),
                                (const uint8_t *)token, strlen(token));
}

#ifdef __cplusplus
}
#endif
