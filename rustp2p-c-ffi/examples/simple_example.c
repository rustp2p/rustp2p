#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include "../rustp2p.h"

void print_error(const char* msg, int code) {
    fprintf(stderr, "%s: error code %d\n", msg, code);
}

int main(int argc, char* argv[]) {
    if (argc < 3) {
        printf("Usage: %s <local_ip> <port> [peer_addr]\n", argv[0]);
        printf("Example: %s 10.0.0.1 8080 udp://127.0.0.1:9090\n", argv[0]);
        return 1;
    }

    const char* local_ip = argv[1];
    uint16_t port = (uint16_t)atoi(argv[2]);
    const char* peer_addr = argc > 3 ? argv[3] : NULL;

    printf("Starting rustp2p node...\n");
    printf("  Local IP: %s\n", local_ip);
    printf("  Port: %u\n", port);
    if (peer_addr) {
        printf("  Peer: %s\n", peer_addr);
    }

    // Create builder
    Rustp2pBuilder* builder = rustp2p_builder_new();
    if (!builder) {
        fprintf(stderr, "Failed to create builder\n");
        return 1;
    }

    // Configure builder
    int ret;
    ret = rustp2p_builder_node_id(builder, local_ip);
    if (ret != RUSTP2P_OK) {
        print_error("Failed to set node ID", ret);
        rustp2p_builder_free(builder);
        return 1;
    }

    ret = rustp2p_builder_udp_port(builder, port);
    if (ret != RUSTP2P_OK) {
        print_error("Failed to set UDP port", ret);
        rustp2p_builder_free(builder);
        return 1;
    }

    ret = rustp2p_builder_tcp_port(builder, port);
    if (ret != RUSTP2P_OK) {
        print_error("Failed to set TCP port", ret);
        rustp2p_builder_free(builder);
        return 1;
    }

    ret = rustp2p_builder_group_code(builder, 12345);
    if (ret != RUSTP2P_OK) {
        print_error("Failed to set group code", ret);
        rustp2p_builder_free(builder);
        return 1;
    }

    ret = rustp2p_builder_encryption(builder, RUSTP2P_ALGO_AES_GCM, "password");
    if (ret != RUSTP2P_OK) {
        print_error("Failed to set encryption", ret);
        rustp2p_builder_free(builder);
        return 1;
    }

    if (peer_addr) {
        ret = rustp2p_builder_add_peer(builder, peer_addr);
        if (ret != RUSTP2P_OK) {
            print_error("Failed to add peer", ret);
            rustp2p_builder_free(builder);
            return 1;
        }
    }

    // Build endpoint
    printf("Building endpoint...\n");
    Rustp2pEndpoint* endpoint = rustp2p_builder_build(builder);
    if (!endpoint) {
        fprintf(stderr, "Failed to build endpoint\n");
        return 1;
    }
    printf("Endpoint built successfully!\n");

    // Example: send a message if we have a peer
    if (argc > 4) {
        const char* dest_ip = argv[4];
        const char* message = "Hello from C FFI!";
        printf("Sending message to %s: %s\n", dest_ip, message);
        ret = rustp2p_endpoint_send_to(endpoint, dest_ip, 
                                       (const uint8_t*)message, strlen(message));
        if (ret == RUSTP2P_OK) {
            printf("Message sent successfully!\n");
        } else {
            print_error("Failed to send message", ret);
        }
    }

    // Receive loop
    printf("Waiting for messages (press Ctrl+C to exit)...\n");
    for (int i = 0; i < 10; i++) {  // Limit iterations for example
        Rustp2pRecvData* recv_data = rustp2p_endpoint_try_recv_from(endpoint);
        if (recv_data) {
            // Get payload
            const uint8_t* data;
            size_t len;
            ret = rustp2p_recv_data_get_payload(recv_data, &data, &len);
            if (ret == RUSTP2P_OK) {
                // Get source IP
                char src_ip[16];
                ret = rustp2p_recv_data_get_src_id(recv_data, src_ip, sizeof(src_ip));
                if (ret == RUSTP2P_OK) {
                    printf("Received from %s (%zu bytes): ", src_ip, len);
                    fwrite(data, 1, len, stdout);
                    printf("\n");
                }
            }
            rustp2p_recv_data_free(recv_data);
        }
        sleep(1);
    }

    // Cleanup
    printf("Cleaning up...\n");
    rustp2p_endpoint_free(endpoint);
    printf("Done!\n");

    return 0;
}
