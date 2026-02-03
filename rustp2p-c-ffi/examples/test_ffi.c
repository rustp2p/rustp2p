#include <stdio.h>
#include <stdlib.h>
#include <assert.h>
#include "../rustp2p.h"

void test_builder_creation() {
    printf("Testing builder creation...\n");
    Rustp2pBuilder* builder = rustp2p_builder_new();
    assert(builder != NULL);
    rustp2p_builder_free(builder);
    printf("✓ Builder creation test passed\n");
}

void test_builder_configuration() {
    printf("Testing builder configuration...\n");
    Rustp2pBuilder* builder = rustp2p_builder_new();
    assert(builder != NULL);

    int ret;
    
    printf("  Setting node_id...\n");
    ret = rustp2p_builder_node_id(builder, "10.0.0.1");
    assert(ret == RUSTP2P_OK);
    
    printf("  Setting udp_port...\n");
    ret = rustp2p_builder_udp_port(builder, 8080);
    assert(ret == RUSTP2P_OK);
    
    printf("  Setting tcp_port...\n");
    ret = rustp2p_builder_tcp_port(builder, 8080);
    assert(ret == RUSTP2P_OK);
    
    printf("  Setting group_code...\n");
    ret = rustp2p_builder_group_code(builder, 12345);
    assert(ret == RUSTP2P_OK);
    
    printf("  Setting encryption...\n");
    ret = rustp2p_builder_encryption(builder, RUSTP2P_ALGO_AES_GCM, "test_password");
    printf("  Encryption result: %d\n", ret);
    assert(ret == RUSTP2P_OK);
    
    printf("  Adding peer...\n");
    ret = rustp2p_builder_add_peer(builder, "udp://127.0.0.1:9090");
    printf("  Add peer result: %d\n", ret);
    assert(ret == RUSTP2P_OK);
    
    rustp2p_builder_free(builder);
    printf("✓ Builder configuration test passed\n");
}

void test_error_handling() {
    printf("Testing error handling...\n");
    
    // Test NULL pointer handling
    int ret = rustp2p_builder_udp_port(NULL, 8080);
    assert(ret == RUSTP2P_ERROR_NULL_PTR);
    
    Rustp2pBuilder* builder = rustp2p_builder_new();
    assert(builder != NULL);
    
    // Test invalid IP
    ret = rustp2p_builder_node_id(builder, "invalid_ip");
    assert(ret == RUSTP2P_ERROR_INVALID_IP);
    
    rustp2p_builder_free(builder);
    printf("✓ Error handling test passed\n");
}

void test_endpoint_build() {
    printf("Testing endpoint build...\n");
    
    Rustp2pBuilder* builder = rustp2p_builder_new();
    assert(builder != NULL);
    
    // Configure with minimal settings
    int ret;
    ret = rustp2p_builder_node_id(builder, "10.0.0.1");
    assert(ret == RUSTP2P_OK);
    
    ret = rustp2p_builder_udp_port(builder, 0);  // Use port 0 for random port
    assert(ret == RUSTP2P_OK);
    
    ret = rustp2p_builder_tcp_port(builder, 0);
    assert(ret == RUSTP2P_OK);
    
    ret = rustp2p_builder_group_code(builder, 12345);
    assert(ret == RUSTP2P_OK);
    
    ret = rustp2p_builder_encryption(builder, RUSTP2P_ALGO_AES_GCM, "password");
    assert(ret == RUSTP2P_OK);
    
    // Build endpoint
    Rustp2pEndpoint* endpoint = rustp2p_builder_build(builder);
    assert(endpoint != NULL);
    
    // Clean up
    rustp2p_endpoint_free(endpoint);
    printf("✓ Endpoint build test passed\n");
}

int main() {
    printf("Running C FFI tests...\n\n");
    
    test_builder_creation();
    test_builder_configuration();
    test_error_handling();
    test_endpoint_build();
    
    printf("\n✓ All tests passed!\n");
    return 0;
}
