/**
 * @file stream_sender.c
 * @brief UDP stream sender for CSI frames — plain blocking sendto().
 */

#include "stream_sender.h"
#include <string.h>
#include <errno.h>
#include "esp_log.h"
#include "lwip/sockets.h"
#include "lwip/netdb.h"
#include "sdkconfig.h"

static const char *TAG = "stream_sender";
static int s_sock = -1;
static struct sockaddr_in s_dest_addr;

static int sender_init_internal(const char *ip, uint16_t port)
{
    /* E-1 fix: close any previously-opened socket before creating a new one.
     * Without this, repeated calls to stream_sender_init_with() leak the old
     * file descriptor (lwIP's CONFIG_LWIP_MAX_SOCKETS is only 16 on C5). */
    if (s_sock >= 0) { close(s_sock); s_sock = -1; }

    s_sock = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (s_sock < 0) { ESP_LOGE(TAG, "socket errno %d", errno); return -1; }

    /* H-1 fix: Set send timeout. For burst mode we use a short 10 ms
     * timeout — individual packet loss is acceptable for CSI streams.
     * The flush timer drains the ring and retries next cycle. */
    struct timeval tv = {
        .tv_sec  = 0,
        .tv_usec = 50 * 1000,  /* 50 ms — balances latency vs. reliability on 384KB SRAM (was 10ms) */
    };
    /* M-3 fix: Check setsockopt return — if it fails, sends may still block
     * but we log the issue rather than silently ignoring it. */
    if (setsockopt(s_sock, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv)) < 0) {
        ESP_LOGW(TAG, "setsockopt(SO_SNDTIMEO) failed: errno %d", errno);
    }

    /* Burst mode: larger send buffer to accommodate multi-packet flush.
     * 64 KB is safe on C5 with PSRAM relieving SRAM pressure. */
    int sndbuf = 64 * 1024;
    if (setsockopt(s_sock, SOL_SOCKET, SO_SNDBUF, &sndbuf, sizeof(sndbuf)) < 0) {
        ESP_LOGW(TAG, "setsockopt(SO_SNDBUF) failed: errno %d", errno);
    }

    memset(&s_dest_addr, 0, sizeof(s_dest_addr));
    s_dest_addr.sin_family = AF_INET;
    s_dest_addr.sin_port = htons(port);
    if (inet_pton(AF_INET, ip, &s_dest_addr.sin_addr) <= 0) {
        ESP_LOGE(TAG, "inet_pton: %s", ip);
        close(s_sock); s_sock = -1; return -1;
    }

    ESP_LOGI(TAG, "UDP ready: %s:%d (send timeout 50 ms)", ip, port);
    return 0;
}

int stream_sender_init(void)
    { return sender_init_internal(CONFIG_CSI_TARGET_IP, CONFIG_CSI_TARGET_PORT); }
int stream_sender_init_with(const char *ip, uint16_t port)
    { return sender_init_internal(ip, port); }

int stream_sender_send(const uint8_t *data, size_t len)
{
    if (s_sock < 0) return -1;
    return sendto(s_sock, data, len, 0,
                  (struct sockaddr *)&s_dest_addr, sizeof(s_dest_addr));
}

void stream_sender_deinit(void)
{
    if (s_sock >= 0) { close(s_sock); s_sock = -1; }
}
