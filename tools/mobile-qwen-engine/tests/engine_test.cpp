// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "journal_mobile_qwen.h"
#include "common.h"
#include "server-embedded.h"
#include "server-http.h"
#include "server-queue.h"
#include "httplib.h"

#include <atomic>
#include <csignal>
#include <sys/socket.h>
#include <unistd.h>
#include <cassert>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <thread>
#include <vector>

namespace httplib::detail {
ssize_t send_socket(socket_t sock, const void * ptr, size_t size, int flags);
}

namespace {
volatile std::sig_atomic_t pipe_signals = 0;
void app_sigpipe(int) { pipe_signals = 1; }
const char * token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
void test_control_lifetime() {
    server_embedded_control control;
    int calls = 0;
    control.request_stop();
    std::atomic<bool> http_ready{false};
    assert(!control.publish_ready(1234, http_ready));
    assert(!http_ready.load());
    control.bind_stop([&] { ++calls; });
    assert(calls == 1);
    control.clear_stop();
    control.request_stop();
    assert(calls == 1);
    // Stop never retains a callback after its referenced lifetime ends.
    for (int generation = 0; generation != 100; ++generation) {
        server_embedded_control current;
        std::atomic<int> observed{0};
        current.bind_stop([&] { observed.fetch_add(1); http_ready.store(false); });
        http_ready.store(false);
        current.publish_ready(1234, http_ready);
        std::thread stopper([&] { current.request_stop(); });
        current.clear_stop();
        stopper.join();
        assert(observed.load() <= 1);
        assert(current.cancelled.load());
    }
    // Publishing ready and stopping share a lifetime boundary. Stop must win
    // even when it overlaps the model-loaded publication.
    for (int generation = 0; generation != 100; ++generation) {
        server_embedded_control current;
        std::atomic<bool> ready{false};
        current.bind_stop([&] { ready.store(false); });
        std::thread publisher([&] { current.publish_ready(1234, ready); });
        current.request_stop();
        publisher.join();
        assert(!ready.load());
        current.clear_stop();
    }
    // Regression: upstream start_loop used to overwrite a pre-start terminate.
    server_queue queue;
    queue.terminate();
    queue.start_loop();
}
void test_http_contract() {
    const auto prior_sigpipe = std::signal(SIGPIPE, app_sigpipe);
    common_params params;
    params.hostnames = {"127.0.0.1"};
    params.port = 0;
    params.api_keys = {token};
    params.n_threads_http = 2;
    server_http_context http;
    assert(http.init(params, true));
    assert(std::signal(SIGPIPE, app_sigpipe) == app_sigpipe);
    http.get("/health", [](const server_http_req &) { return std::make_unique<server_http_res>(); });
    http.post("/v1/chat/completions", [](const server_http_req &) { return std::make_unique<server_http_res>(); });
    assert(http.start());
    assert(http.port > 0 && http.port <= 65535);
    httplib::Client client("127.0.0.1", http.port);
    client.set_connection_timeout(2);
    client.set_read_timeout(2);
    auto unauthorized = client.Get("/health");
    assert(unauthorized && unauthorized->status == 401);
    httplib::Headers auth{{"Authorization", std::string("Bearer ") + token}};
    auto stale = client.Get("/health", httplib::Headers{{"Authorization", "Bearer stale-generation-secret"}});
    assert(stale && stale->status == 401);
    auto loading = client.Get("/health", auth);
    assert(loading && loading->status == 503);
    http.is_ready.store(true);
    auto ready = client.Get("/health", auth);
    assert(ready && ready->status == 200);
    auto unauthenticated_inference = client.Post("/v1/chat/completions", "{}", "application/json");
    assert(unauthenticated_inference && unauthenticated_inference->status == 401);
    http.is_ready.store(false);
    http.stop();
    http.join();
    httplib::Client retired("127.0.0.1", http.port);
    retired.set_connection_timeout(1);
    assert(!retired.Get("/health", auth));
#ifdef MSG_NOSIGNAL
    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
    close(sockets[1]);
    assert(httplib::detail::send_socket(sockets[0], "closed peer", 11, 0) < 0);
    close(sockets[0]);
    assert(pipe_signals == 0);
#endif
    assert(std::signal(SIGPIPE, prior_sigpipe) == app_sigpipe);
}
void test_abi(const std::filesystem::path & model) {
    assert(journal_mobile_qwen_abi_version() == 1);
    journal_mobile_qwen_config_v1 config{1, sizeof(config), 1,
        journal_mobile_qwen_runtime_identity(), journal_mobile_qwen_source_revision(),
        "verified-test-generation", model.c_str(), model.c_str(), token};
    char error[512]{};
    journal_mobile_qwen_handle * handle = nullptr;
    config.abi_version = 2;
    assert(journal_mobile_qwen_create_v1(&config, &handle, error, sizeof(error)) == JOURNAL_MOBILE_QWEN_INVALID);
    assert(!handle);
    config.abi_version = 1;
    config.expected_source_revision = "wrong";
    assert(journal_mobile_qwen_create_v1(&config, &handle, error, sizeof(error)) == JOURNAL_MOBILE_QWEN_INVALID);
    config.expected_source_revision = journal_mobile_qwen_source_revision();
    config.bearer_token = "short";
    assert(journal_mobile_qwen_create_v1(&config, &handle, error, sizeof(error)) == JOURNAL_MOBILE_QWEN_INVALID);
    config.bearer_token = token;
    for (uint64_t generation = 1; generation <= 3; ++generation) {
        config.generation = generation;
        assert(journal_mobile_qwen_create_v1(&config, &handle, error, sizeof(error)) == JOURNAL_MOBILE_QWEN_OK);
        journal_mobile_qwen_handle * duplicate = nullptr;
        assert(journal_mobile_qwen_create_v1(&config, &duplicate, error, sizeof(error)) == JOURNAL_MOBILE_QWEN_BUSY);
        assert(!duplicate);
        assert(journal_mobile_qwen_request_stop_v1(handle) == JOURNAL_MOBILE_QWEN_OK);
        assert(journal_mobile_qwen_run_v1(handle) == JOURNAL_MOBILE_QWEN_OK);
        assert(journal_mobile_qwen_run_v1(handle) == JOURNAL_MOBILE_QWEN_BUSY);
        journal_mobile_qwen_snapshot_v1 snapshot{1, sizeof(snapshot)};
        assert(journal_mobile_qwen_get_snapshot_v1(handle, &snapshot) == JOURNAL_MOBILE_QWEN_OK);
        assert(snapshot.state == JOURNAL_MOBILE_QWEN_STOPPED && snapshot.bound_port == 0 && snapshot.generation == generation);
        assert(journal_mobile_qwen_destroy_v1(handle) == JOURNAL_MOBILE_QWEN_OK);
    }
    // Use the real server's failure path twice: listener/GC/model/backend cleanup
    // must finish before a later generation can be admitted. No large model used.
    for (int attempt = 0; attempt != 2; ++attempt) {
        assert(journal_mobile_qwen_create_v1(&config, &handle, error, sizeof(error)) == JOURNAL_MOBILE_QWEN_OK);
        assert(journal_mobile_qwen_run_v1(handle) == JOURNAL_MOBILE_QWEN_ENGINE_ERROR);
        journal_mobile_qwen_snapshot_v1 snapshot{1, sizeof(snapshot)};
        assert(journal_mobile_qwen_get_snapshot_v1(handle, &snapshot) == JOURNAL_MOBILE_QWEN_OK);
        assert(snapshot.state == JOURNAL_MOBILE_QWEN_FAILED && snapshot.bound_port == 0);
        assert(std::strstr(snapshot.error, token) == nullptr);
        assert(journal_mobile_qwen_destroy_v1(handle) == JOURNAL_MOBILE_QWEN_OK);
    }
}
}
int main() {
    test_control_lifetime();
    test_http_contract();
    const auto path = std::filesystem::temp_directory_path() / ("journal-qwen-test-" + std::to_string(reinterpret_cast<uintptr_t>(&token)));
    { std::ofstream file(path); file << "invalid GGUF; native failure path"; }
    test_abi(path);
    std::filesystem::remove(path);
}
