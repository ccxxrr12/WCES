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
    s_sock = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (s_sock < 0) { ESP_LOGE(TAG, "socket errno %d", errno); return -1; }

    memset(&s_dest_addr, 0, sizeof(s_dest_addr));
    s_dest_addr.sin_family = AF_INET;
    s_dest_addr.sin_port = htons(port);
    if (inet_pton(AF_INET, ip, &s_dest_addr.sin_addr) <= 0) {
        ESP_LOGE(TAG, "inet_pton: %s", ip);
        close(s_sock); s_sock = -1; return -1;
    }

    ESP_LOGI(TAG, "UDP ready: %s:%d", ip, port);
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
