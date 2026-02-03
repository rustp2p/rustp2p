#ifndef RUSTP2P_H
#define RUSTP2P_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque types
typedef struct Rustp2pBuilder Rustp2pBuilder;
typedef struct Rustp2pEndpoint Rustp2pEndpoint;
typedef struct Rustp2pRecvData Rustp2pRecvData;

// Error codes
#define RUSTP2P_OK 0
#define RUSTP2P_ERROR -1
#define RUSTP2P_ERROR_NULL_PTR -2
#define RUSTP2P_ERROR_INVALID_STR -3
#define RUSTP2P_ERROR_INVALID_IP -4
#define RUSTP2P_ERROR_BUILD_FAILED -5
#define RUSTP2P_ERROR_WOULD_BLOCK -6
#define RUSTP2P_ERROR_EOF -7

// Encryption algorithms
#define RUSTP2P_ALGO_AES_GCM 0
#define RUSTP2P_ALGO_CHACHA20_POLY1305 1

// Builder functions
/**
 * Create a new builder instance
 * @return Pointer to builder, or NULL on failure
 */
Rustp2pBuilder* rustp2p_builder_new(void);

/**
 * Set UDP port for the builder
 * @param builder Pointer to builder
 * @param port UDP port number
 * @return RUSTP2P_OK on success, error code on failure
 */
int rustp2p_builder_udp_port(Rustp2pBuilder* builder, uint16_t port);

/**
 * Set TCP port for the builder
 * @param builder Pointer to builder
 * @param port TCP port number
 * @return RUSTP2P_OK on success, error code on failure
 */
int rustp2p_builder_tcp_port(Rustp2pBuilder* builder, uint16_t port);

/**
 * Set node ID from IPv4 address string
 * @param builder Pointer to builder
 * @param node_id_str IPv4 address as string (e.g., "10.0.0.1")
 * @return RUSTP2P_OK on success, error code on failure
 */
int rustp2p_builder_node_id(Rustp2pBuilder* builder, const char* node_id_str);

/**
 * Set group code
 * @param builder Pointer to builder
 * @param group_code Group code (peers with same code can connect)
 * @return RUSTP2P_OK on success, error code on failure
 */
int rustp2p_builder_group_code(Rustp2pBuilder* builder, uint32_t group_code);

/**
 * Add a peer address
 * @param builder Pointer to builder
 * @param peer_addr Peer address string (e.g., "udp://127.0.0.1:9090" or "tcp://192.168.1.1:8080")
 * @return RUSTP2P_OK on success, error code on failure
 */
int rustp2p_builder_add_peer(Rustp2pBuilder* builder, const char* peer_addr);

/**
 * Set encryption algorithm and password
 * @param builder Pointer to builder
 * @param algorithm Encryption algorithm (RUSTP2P_ALGO_AES_GCM or RUSTP2P_ALGO_CHACHA20_POLY1305)
 * @param password Encryption password
 * @return RUSTP2P_OK on success, error code on failure
 */
int rustp2p_builder_encryption(Rustp2pBuilder* builder, int algorithm, const char* password);

/**
 * Build the endpoint from the builder
 * @param builder Pointer to builder (will be consumed/freed)
 * @return Pointer to endpoint, or NULL on failure
 */
Rustp2pEndpoint* rustp2p_builder_build(Rustp2pBuilder* builder);

/**
 * Free/destroy the builder
 * @param builder Pointer to builder
 */
void rustp2p_builder_free(Rustp2pBuilder* builder);

// Endpoint functions
/**
 * Send data to a peer (blocking)
 * @param endpoint Pointer to endpoint
 * @param dest_ip Destination node IPv4 address string (e.g., "10.0.0.2")
 * @param data Pointer to data buffer
 * @param len Length of data
 * @return RUSTP2P_OK on success, error code on failure
 */
int rustp2p_endpoint_send_to(Rustp2pEndpoint* endpoint, const char* dest_ip, 
                              const uint8_t* data, size_t len);

/**
 * Try to send data to a peer (non-blocking)
 * @param endpoint Pointer to endpoint
 * @param dest_ip Destination node IPv4 address string (e.g., "10.0.0.2")
 * @param data Pointer to data buffer
 * @param len Length of data
 * @return RUSTP2P_OK on success, RUSTP2P_ERROR_WOULD_BLOCK if would block, error code on failure
 */
int rustp2p_endpoint_try_send_to(Rustp2pEndpoint* endpoint, const char* dest_ip,
                                  const uint8_t* data, size_t len);

/**
 * Receive data from peers (blocking)
 * @param endpoint Pointer to endpoint
 * @return Pointer to received data, or NULL on error. Must be freed with rustp2p_recv_data_free
 */
Rustp2pRecvData* rustp2p_endpoint_recv_from(Rustp2pEndpoint* endpoint);

/**
 * Try to receive data from peers (non-blocking)
 * @param endpoint Pointer to endpoint
 * @return Pointer to received data, or NULL if no data available or on error. 
 *         Must be freed with rustp2p_recv_data_free
 */
Rustp2pRecvData* rustp2p_endpoint_try_recv_from(Rustp2pEndpoint* endpoint);

/**
 * Free/destroy the endpoint
 * @param endpoint Pointer to endpoint
 */
void rustp2p_endpoint_free(Rustp2pEndpoint* endpoint);

// Received data functions
/**
 * Get payload data from received data
 * @param recv_data Pointer to received data
 * @param out_data Output pointer to data (borrowed, don't free)
 * @param out_len Output length
 * @return RUSTP2P_OK on success, error code on failure
 */
int rustp2p_recv_data_get_payload(const Rustp2pRecvData* recv_data, 
                                   const uint8_t** out_data, size_t* out_len);

/**
 * Get source node ID from received data as IPv4 string
 * @param recv_data Pointer to received data
 * @param buffer Output buffer for IP string
 * @param buffer_len Size of buffer (should be at least 16 bytes)
 * @return RUSTP2P_OK on success, error code on failure
 */
int rustp2p_recv_data_get_src_id(const Rustp2pRecvData* recv_data, 
                                  char* buffer, size_t buffer_len);

/**
 * Get destination node ID from received data as IPv4 string
 * @param recv_data Pointer to received data
 * @param buffer Output buffer for IP string
 * @param buffer_len Size of buffer (should be at least 16 bytes)
 * @return RUSTP2P_OK on success, error code on failure
 */
int rustp2p_recv_data_get_dest_id(const Rustp2pRecvData* recv_data,
                                   char* buffer, size_t buffer_len);

/**
 * Check if the received data was relayed
 * @param recv_data Pointer to received data
 * @return 1 if relayed, 0 if direct, negative on error
 */
int rustp2p_recv_data_is_relay(const Rustp2pRecvData* recv_data);

/**
 * Free received data
 * @param recv_data Pointer to received data
 */
void rustp2p_recv_data_free(Rustp2pRecvData* recv_data);

#ifdef __cplusplus
}
#endif

#endif // RUSTP2P_H
