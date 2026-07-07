/**
 * @file rvf_parser.c
 * @brief RVF container parser — validates header, manifest, and build hash.
 *
 * The parser works entirely on a contiguous byte buffer (no heap allocation).
 * All pointers in rvf_parsed_t point into the caller's buffer.
 */

#include "rvf_parser.h"

#include <string.h>
#include "esp_log.h"
#define MBEDTLS_DECLARE_PRIVATE_IDENTIFIERS
#include "mbedtls/private/sha256.h"
#include "mbedtls/md.h"

static const char *TAG = "rvf";

bool rvf_is_rvf(const uint8_t *data, uint32_t data_len)
{
    if (data == NULL || data_len < 4) return false;
    uint32_t magic;
    memcpy(&magic, data, sizeof(magic));
    return magic == RVF_MAGIC;
}

bool rvf_is_raw_wasm(const uint8_t *data, uint32_t data_len)
{
    if (data == NULL || data_len < 4) return false;
    uint32_t magic;
    memcpy(&magic, data, sizeof(magic));
    return magic == WASM_BINARY_MAGIC;
}

esp_err_t rvf_parse(const uint8_t *data, uint32_t data_len, rvf_parsed_t *out)
{
    if (data == NULL || out == NULL) return ESP_ERR_INVALID_ARG;

    memset(out, 0, sizeof(rvf_parsed_t));

    /* Minimum size: header + manifest + at least 8 bytes WASM ("\0asm" + version). */
    if (data_len < RVF_HEADER_SIZE + RVF_MANIFEST_SIZE + 8) {
        ESP_LOGE(TAG, "RVF too small: %lu bytes", (unsigned long)data_len);
        return ESP_ERR_INVALID_SIZE;
    }

    /* ---- Parse header ---- */
    const rvf_header_t *hdr = (const rvf_header_t *)data;

    if (hdr->magic != RVF_MAGIC) {
        ESP_LOGE(TAG, "Bad RVF magic: 0x%08lx", (unsigned long)hdr->magic);
        return ESP_ERR_INVALID_STATE;
    }

    if (hdr->format_version != RVF_FORMAT_VERSION) {
        ESP_LOGE(TAG, "Unsupported RVF version: %u (expected %u)",
                 hdr->format_version, RVF_FORMAT_VERSION);
        return ESP_ERR_NOT_SUPPORTED;
    }

    if (hdr->manifest_len != RVF_MANIFEST_SIZE) {
        ESP_LOGE(TAG, "Bad manifest size: %lu (expected %d)",
                 (unsigned long)hdr->manifest_len, RVF_MANIFEST_SIZE);
        return ESP_ERR_INVALID_SIZE;
    }

    if (hdr->wasm_len == 0 || hdr->wasm_len > (128 * 1024)) {
        ESP_LOGE(TAG, "Bad WASM size: %lu", (unsigned long)hdr->wasm_len);
        return ESP_ERR_INVALID_SIZE;
    }

    if (hdr->signature_len != 0 && hdr->signature_len != RVF_SIGNATURE_LEN) {
        ESP_LOGE(TAG, "Bad signature size: %lu", (unsigned long)hdr->signature_len);
        return ESP_ERR_INVALID_SIZE;
    }

    /* C-12 fix: Bound test_vectors_len to prevent integer overflow in the
     * total size computation below. Without this, a crafted header with a
     * huge test_vectors_len could cause expected_total to wrap around to a
     * small value, bypassing the truncation check and leading to
     * out-of-bounds reads when locating sections. */
    if (hdr->test_vectors_len > (128 * 1024)) {
        ESP_LOGE(TAG, "Bad test_vectors size: %lu", (unsigned long)hdr->test_vectors_len);
        return ESP_ERR_INVALID_SIZE;
    }

    /* Verify total_len consistency.
     * Each component is now individually bounded (wasm_len ≤ 128 KB,
     * signature_len ∈ {0, 64}, test_vectors_len ≤ 128 KB), so the sum
     * cannot overflow uint32_t (max ~256 KB + header + manifest). */
    uint32_t expected_total = RVF_HEADER_SIZE + RVF_MANIFEST_SIZE
                            + hdr->wasm_len + hdr->signature_len
                            + hdr->test_vectors_len;
    if (hdr->total_len != expected_total) {
        ESP_LOGE(TAG, "RVF total_len mismatch: %lu != %lu",
                 (unsigned long)hdr->total_len, (unsigned long)expected_total);
        return ESP_ERR_INVALID_SIZE;
    }

    if (data_len < expected_total) {
        ESP_LOGE(TAG, "RVF truncated: have %lu, need %lu",
                 (unsigned long)data_len, (unsigned long)expected_total);
        return ESP_ERR_INVALID_SIZE;
    }

    /* ---- Locate sections ---- */
    uint32_t offset = RVF_HEADER_SIZE;

    const rvf_manifest_t *manifest = (const rvf_manifest_t *)(data + offset);
    offset += RVF_MANIFEST_SIZE;

    const uint8_t *wasm_data = data + offset;
    offset += hdr->wasm_len;

    const uint8_t *signature = NULL;
    if (hdr->signature_len > 0) {
        signature = data + offset;
        offset += hdr->signature_len;
    }

    const uint8_t *test_vectors = NULL;
    uint32_t tvec_len = 0;
    if (hdr->test_vectors_len > 0) {
        test_vectors = data + offset;
        tvec_len = hdr->test_vectors_len;
    }

    /* ---- Validate manifest ---- */
    if (manifest->required_host_api > RVF_HOST_API_V1) {
        ESP_LOGE(TAG, "Module requires host API v%u, we support v%u",
                 manifest->required_host_api, RVF_HOST_API_V1);
        return ESP_ERR_NOT_SUPPORTED;
    }

    /* Ensure module_name is null-terminated. */
    if (manifest->module_name[31] != '\0') {
        ESP_LOGE(TAG, "Module name not null-terminated");
        return ESP_ERR_INVALID_STATE;
    }

    /* ---- Verify build hash (SHA-256 of WASM payload) ---- */
    uint8_t computed_hash[32];
    int ret = mbedtls_sha256(wasm_data, hdr->wasm_len, computed_hash, 0);
    if (ret != 0) {
        ESP_LOGE(TAG, "SHA-256 computation failed: %d", ret);
        return ESP_FAIL;
    }

    if (memcmp(computed_hash, manifest->build_hash, 32) != 0) {
        ESP_LOGE(TAG, "Build hash mismatch — WASM payload corrupted or tampered");
        return ESP_ERR_INVALID_CRC;
    }

    /* ---- Verify WASM payload starts with WASM magic ---- */
    if (hdr->wasm_len >= 4) {
        uint32_t wasm_magic;
        memcpy(&wasm_magic, wasm_data, sizeof(wasm_magic));
        if (wasm_magic != WASM_BINARY_MAGIC) {
            ESP_LOGE(TAG, "WASM payload has bad magic: 0x%08lx",
                     (unsigned long)wasm_magic);
            return ESP_ERR_INVALID_STATE;
        }
    }

    /* ---- Fill output ---- */
    out->header       = hdr;
    out->manifest     = manifest;
    out->wasm_data    = wasm_data;
    out->wasm_len     = hdr->wasm_len;
    out->signature    = signature;
    out->test_vectors = test_vectors;
    out->test_vectors_len = tvec_len;

    ESP_LOGI(TAG, "RVF parsed: \"%s\" v%u, wasm=%lu bytes, caps=0x%04lx, "
             "budget=%lu us, signed=%s",
             manifest->module_name,
             manifest->required_host_api,
             (unsigned long)hdr->wasm_len,
             (unsigned long)manifest->capabilities,
             (unsigned long)manifest->max_frame_us,
             signature ? "yes" : "no");

    return ESP_OK;
}

esp_err_t rvf_verify_signature(const rvf_parsed_t *parsed, const uint8_t *data,
                                const uint8_t *pubkey)
{
    if (parsed == NULL || data == NULL || pubkey == NULL) {
        return ESP_ERR_INVALID_ARG;
    }

    if (parsed->signature == NULL) {
        ESP_LOGE(TAG, "No signature in RVF");
        return ESP_ERR_NOT_FOUND;
    }

    /* Signature covers: header + manifest + wasm payload. */
    uint32_t signed_len = RVF_HEADER_SIZE + RVF_MANIFEST_SIZE + parsed->wasm_len;

    /*
     * C-11 fix: HMAC-SHA256 integrity check (TEMPORARY — NOT Ed25519).
     *
     * The previous implementation computed SHA-256(pubkey || signed_region),
     * which is a plain hash that anyone can recompute given the public key —
     * it provided NO authentication and NO forgery resistance (an attacker
     * who knows the pubkey can forge a valid "signature").
     *
     * This temporary scheme treats the 32-byte "pubkey" parameter as a
     * shared HMAC secret and verifies:
     *   signature[0..31] == HMAC-SHA256(secret, header ‖ manifest ‖ wasm)
     *
     * HMAC provides proper key-binding: without the secret, an attacker
     * cannot produce a valid tag even if they can choose the message.
     *
     * TODO: SECURITY — implement real Ed25519 verification.
     * ESP-IDF v5.3+ supports CONFIG_MBEDTLS_EDDSA_C; alternatively link
     * TweetNaCl. The RVF builder MUST be updated to sign with the matching
     * Ed25519 scheme when this is deployed in production.
     */
    const mbedtls_md_info_t *md_info = mbedtls_md_info_from_type(MBEDTLS_MD_SHA256);
    if (md_info == NULL) {
        ESP_LOGE(TAG, "mbedtls_md_info_from_type(SHA256) failed");
        return ESP_FAIL;
    }

    uint8_t expected[32];
    int ret = mbedtls_md_hmac(md_info, pubkey, 32, data, signed_len, expected);
    if (ret != 0) {
        ESP_LOGE(TAG, "HMAC-SHA256 computation failed: -0x%04x", -ret);
        return ESP_FAIL;
    }

    /* Constant-time comparison to prevent timing side-channels. */
    volatile uint8_t diff = 0;
    for (int i = 0; i < 32; i++) {
        diff |= parsed->signature[i] ^ expected[i];
    }
    if (diff != 0) {
        ESP_LOGE(TAG, "Signature verification failed — HMAC mismatch or tampered");
        return ESP_ERR_INVALID_CRC;
    }

    ESP_LOGW(TAG, "Signature verified (HMAC-SHA256 — TEMPORARY scheme, NOT Ed25519). "
             "TODO: implement real Ed25519 (CONFIG_MBEDTLS_EDDSA_C or TweetNaCl).");
    return ESP_OK;
}
