/**
 * @file ota_update.c
 * @brief HTTP OTA firmware update for ESP32-C5/S3 CSI Node.
 *
 * Uses ESP-IDF's native OTA API with rollback support.
 * The HTTP server runs on port 8032 and accepts:
 *   POST /ota — firmware binary payload (application/octet-stream)
 *   GET /ota/status — current firmware version and partition info
 */

#include "ota_update.h"
#include "psk_auth.h"

#include <string.h>
#include "esp_log.h"
#include "esp_ota_ops.h"
#include "esp_http_server.h"
#include "esp_app_desc.h"
#include "nvs_flash.h"
#include "nvs.h"

static const char *TAG = "ota_update";

/** OTA HTTP server port. */
#define OTA_PORT 8032

/** Maximum firmware size (900 KB — matches CI binary size gate). */
#define OTA_MAX_SIZE (900 * 1024)

/** Cached PSK loaded from NVS at init time. Empty = auth disabled.
 *  NVS namespace/key and max length are defined in psk_auth.h. */
static char s_ota_psk[PSK_AUTH_MAX_LEN] = {0};

/**
 * ADR-050: Verify the Authorization header contains the correct PSK.
 * Returns true only if the Bearer token matches the stored PSK.
 * Rejects all requests when no PSK is provisioned (secure default).
 */
static bool ota_check_auth(httpd_req_t *req)
{
    if (s_ota_psk[0] == '\0') {
        ESP_LOGE(TAG, "OTA auth rejected: no PSK provisioned");
        return false;
    }

    return psk_verify_bearer(req, s_ota_psk);
}

/**
 * GET /ota/status — return firmware version and partition info.
 */
static esp_err_t ota_status_handler(httpd_req_t *req)
{
    const esp_app_desc_t *app = esp_app_get_description();
    const esp_partition_t *running = esp_ota_get_running_partition();
    const esp_partition_t *update = esp_ota_get_next_update_partition(NULL);

    char response[512];
    int len = snprintf(response, sizeof(response),
        "{\"version\":\"%s\",\"date\":\"%s\",\"time\":\"%s\","
        "\"running_partition\":\"%s\",\"next_partition\":\"%s\","
        "\"max_size\":%d}",
        app->version, app->date, app->time,
        running ? running->label : "unknown",
        update ? update->label : "none",
        OTA_MAX_SIZE);

    httpd_resp_set_type(req, "application/json");
    httpd_resp_send(req, response, len);
    return ESP_OK;
}

/**
 * POST /ota — receive and flash firmware binary.
 */
static esp_err_t ota_upload_handler(httpd_req_t *req)
{
    /* ADR-050: Authenticate before accepting firmware upload. */
    if (!ota_check_auth(req)) {
        ESP_LOGW(TAG, "OTA upload rejected: authentication failed");
        httpd_resp_send_err(req, HTTPD_403_FORBIDDEN,
                            "Authentication required. Use: Authorization: Bearer <psk>");
        return ESP_FAIL;
    }

    ESP_LOGI(TAG, "OTA update started, content_length=%d", req->content_len);

    if (req->content_len <= 0 || req->content_len > OTA_MAX_SIZE) {
        httpd_resp_send_err(req, HTTPD_400_BAD_REQUEST,
                            "Invalid firmware size (must be 1B - 900KB)");
        return ESP_FAIL;
    }

    const esp_partition_t *update_partition = esp_ota_get_next_update_partition(NULL);
    if (update_partition == NULL) {
        httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR,
                            "No OTA partition available");
        return ESP_FAIL;
    }

    esp_ota_handle_t ota_handle;
    esp_err_t err = esp_ota_begin(update_partition, OTA_WITH_SEQUENTIAL_WRITES, &ota_handle);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "esp_ota_begin failed: %s", esp_err_to_name(err));
        httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR,
                            "OTA begin failed");
        return ESP_FAIL;
    }

    /* Read firmware in chunks. */
    char buf[1024];
    int received = 0;
    int total = 0;

    while (total < req->content_len) {
        received = httpd_req_recv(req, buf, sizeof(buf));
        if (received <= 0) {
            if (received == HTTPD_SOCK_ERR_TIMEOUT) {
                continue;  /* Retry on timeout. */
            }
            ESP_LOGE(TAG, "OTA receive error at byte %d", total);
            esp_ota_abort(ota_handle);
            httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR,
                                "Receive error");
            return ESP_FAIL;
        }

        err = esp_ota_write(ota_handle, buf, received);
        if (err != ESP_OK) {
            ESP_LOGE(TAG, "esp_ota_write failed at byte %d: %s",
                     total, esp_err_to_name(err));
            esp_ota_abort(ota_handle);
            httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR,
                                "OTA write failed");
            return ESP_FAIL;
        }

        total += received;
        if ((total % (64 * 1024)) == 0) {
            ESP_LOGI(TAG, "OTA progress: %d / %d bytes (%.0f%%)",
                     total, req->content_len,
                     (float)total * 100.0f / (float)req->content_len);
        }
    }

    err = esp_ota_end(ota_handle);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "esp_ota_end failed: %s", esp_err_to_name(err));
        httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR,
                            "OTA validation failed");
        return ESP_FAIL;
    }

    err = esp_ota_set_boot_partition(update_partition);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "esp_ota_set_boot_partition failed: %s", esp_err_to_name(err));
        httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR,
                            "Set boot partition failed");
        return ESP_FAIL;
    }

    ESP_LOGI(TAG, "OTA update successful! Rebooting to partition '%s'...",
             update_partition->label);

    const char *resp = "{\"status\":\"ok\",\"message\":\"OTA update successful. Rebooting...\"}";
    httpd_resp_set_type(req, "application/json");
    httpd_resp_send(req, resp, strlen(resp));

    /* Delay briefly to let the response flush, then reboot. */
    vTaskDelay(pdMS_TO_TICKS(1000));
    esp_restart();

    return ESP_OK;  /* Never reached. */
}

/** Internal: start the HTTP server and register OTA endpoints. */
static esp_err_t ota_start_server(httpd_handle_t *out_handle)
{
    httpd_config_t config = HTTPD_DEFAULT_CONFIG();
    config.server_port = OTA_PORT;
    config.max_uri_handlers = 12;  /* Extra slots for WASM endpoints (ADR-040). */
    /* Increase receive timeout for large uploads. */
    config.recv_wait_timeout = 30;

    httpd_handle_t server = NULL;
    esp_err_t err = httpd_start(&server, &config);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "Failed to start OTA HTTP server on port %d: %s",
                 OTA_PORT, esp_err_to_name(err));
        if (out_handle) *out_handle = NULL;
        return err;
    }

    httpd_uri_t status_uri = {
        .uri      = "/ota/status",
        .method   = HTTP_GET,
        .handler  = ota_status_handler,
        .user_ctx = NULL,
    };
    httpd_register_uri_handler(server, &status_uri);

    httpd_uri_t upload_uri = {
        .uri      = "/ota",
        .method   = HTTP_POST,
        .handler  = ota_upload_handler,
        .user_ctx = NULL,
    };
    httpd_register_uri_handler(server, &upload_uri);

    ESP_LOGI(TAG, "OTA HTTP server started on port %d", OTA_PORT);
    ESP_LOGI(TAG, "  GET  /ota/status — firmware version info");
    ESP_LOGI(TAG, "  POST /ota        — upload new firmware binary");

    if (out_handle) *out_handle = server;
    return ESP_OK;
}

esp_err_t ota_update_init(void)
{
    /* ADR-050: Load OTA PSK from NVS if provisioned, or auto-generate
     * one on first boot for security. */
    nvs_handle_t nvs;
    bool psk_found = false;
    if (nvs_open(PSK_AUTH_NVS_NAMESPACE, NVS_READONLY, &nvs) == ESP_OK) {
        size_t len = sizeof(s_ota_psk);
        if (nvs_get_str(nvs, PSK_AUTH_NVS_KEY, s_ota_psk, &len) == ESP_OK) {
            ESP_LOGI(TAG, "OTA PSK loaded from NVS (%d chars) — authentication enabled", (int)len - 1);
            psk_found = true;
        }
        nvs_close(nvs);
    }

    if (!psk_found) {
        /* No PSK provisioned: generate a random 32-hex-char key on first
         * boot and save it to NVS for persistence. Output the key via serial
         * so the admin can capture it once. This prevents the device from
         * being permanently locked out of OTA on field deployments. */
        uint8_t random_bytes[16];
        esp_fill_random(random_bytes, sizeof(random_bytes));
        for (int i = 0; i < 16; i++) {
            snprintf(&s_ota_psk[i * 2], 3, "%02x", random_bytes[i]);
        }
        s_ota_psk[32] = '\0';

        /* Persist to NVS */
        nvs_handle_t nvs_w;
        if (nvs_open(PSK_AUTH_NVS_NAMESPACE, NVS_READWRITE, &nvs_w) == ESP_OK) {
            nvs_set_str(nvs_w, PSK_AUTH_NVS_KEY, s_ota_psk);
            esp_err_t commit_err = nvs_commit(nvs_w);
            nvs_close(nvs_w);
            if (commit_err != ESP_OK) {
                ESP_LOGE(TAG, "Failed to persist auto-generated PSK (%s) — "
                         "key will be lost on reboot. Flash may be full or worn.",
                         esp_err_to_name(commit_err));
            }
        } else {
            ESP_LOGE(TAG, "Cannot open NVS for writing — auto-generated PSK "
                     "will be lost on reboot. Flash may be corrupted.");
        }

        /* Output once via serial — admin must capture this */
        ESP_LOGI(TAG, "==============================================");
        ESP_LOGI(TAG, "AUTO-GENERATED OTA PSK: %s", s_ota_psk);
        ESP_LOGI(TAG, "Use: Authorization: Bearer %s", s_ota_psk);
        ESP_LOGI(TAG, "SAVE THIS KEY — it will NOT be shown again.");
        ESP_LOGI(TAG, "==============================================");
    }

    return ota_start_server(NULL);
}

esp_err_t ota_update_init_ex(void **out_server)
{
    return ota_start_server((httpd_handle_t *)out_server);
}

const char *ota_psk_get(void)
{
    return s_ota_psk;
}
