// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "journal_mobile_qwen.h"
#include "httplib.h"
#include "nlohmann/json.hpp"

#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <thread>
#include <string>

using namespace std::chrono_literals;
using json = nlohmann::json;

int main() {
    const char * model = std::getenv("JOURNAL_MOBILE_QWEN_TEST_MODEL");
    const char * projector = std::getenv("JOURNAL_MOBILE_QWEN_TEST_PROJECTOR");
    if (!model || !projector) { return 77; }
    const char * token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const httplib::Headers auth{{"Authorization", std::string("Bearer ") + token}};
    for (uint64_t generation = 1; generation <= 2; ++generation) {
        journal_mobile_qwen_config_v1 config{1, sizeof(config), generation,
            journal_mobile_qwen_runtime_identity(), journal_mobile_qwen_source_revision(),
            "verified-integration-fixture", model, projector, token};
        journal_mobile_qwen_handle * handle = nullptr;
        char error[512]{};
        if (journal_mobile_qwen_create_v1(&config, &handle, error, sizeof(error)) != JOURNAL_MOBILE_QWEN_OK) { return 1; }
        int engine_result = -1;
        std::thread engine([&] { engine_result = journal_mobile_qwen_run_v1(handle); });
        journal_mobile_qwen_snapshot_v1 snapshot{1, sizeof(snapshot)};
        const auto ready_deadline = std::chrono::steady_clock::now() + 120s;
        do {
            journal_mobile_qwen_get_snapshot_v1(handle, &snapshot);
            if (snapshot.state == JOURNAL_MOBILE_QWEN_READY || snapshot.state == JOURNAL_MOBILE_QWEN_FAILED) { break; }
            std::this_thread::sleep_for(20ms);
        } while (std::chrono::steady_clock::now() < ready_deadline);
        if (snapshot.state != JOURNAL_MOBILE_QWEN_READY) {
            journal_mobile_qwen_request_stop_v1(handle);
            engine.join();
            journal_mobile_qwen_destroy_v1(handle);
            return 1;
        }
        const auto port = snapshot.bound_port;
        httplib::Client client("127.0.0.1", port);
        client.set_read_timeout(90);
        const auto regular = client.Post("/completion", auth,
            R"({"prompt":"Say hello.","n_predict":8,"cache_prompt":false})", "application/json");
        if (!regular || regular->status != 200 || json::parse(regular->body).at("content").get<std::string>().empty()) {
            journal_mobile_qwen_request_stop_v1(handle);
            engine.join();
            journal_mobile_qwen_destroy_v1(handle);
            return 1;
        }
        std::string prompt;
        for (int index = 0; index != 6000; ++index) { prompt += "oak "; }
        const auto body = json{{"prompt", prompt}, {"n_predict", 32}, {"cache_prompt", false}}.dump();
        std::thread inference([&] {
            httplib::Client active("127.0.0.1", port);
            active.set_read_timeout(120);
            active.Post("/completion", auth, body, "application/json");
        });
        bool witnessed_prefill = false;
        const auto prefill_deadline = std::chrono::steady_clock::now() + 90s;
        do {
            const auto slots = client.Get("/slots", auth);
            if (!slots || slots->status != 200) { break; }
            const auto row = json::parse(slots->body).at(0);
            const auto processed = row.value("n_prompt_tokens_processed", 0);
            const auto total = row.value("n_prompt_tokens", 0);
            if (row.at("is_processing").get<bool>() && processed > 0 && processed < total) {
                std::fprintf(stderr, "generation %llu regular completion passed; active prefill %d/%d tokens\n",
                             static_cast<unsigned long long>(generation), processed, total);
                witnessed_prefill = true;
                break;
            }
            std::this_thread::sleep_for(20ms);
        } while (std::chrono::steady_clock::now() < prefill_deadline);
        const auto stopped_at = std::chrono::steady_clock::now();
        journal_mobile_qwen_request_stop_v1(handle);
        engine.join();
        const auto retire_ms = std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now() - stopped_at).count();
        inference.join();
        journal_mobile_qwen_get_snapshot_v1(handle, &snapshot);
        const bool stopped = engine_result == JOURNAL_MOBILE_QWEN_OK && snapshot.state == JOURNAL_MOBILE_QWEN_STOPPED && snapshot.bound_port == 0;
        httplib::Client retired("127.0.0.1", port);
        retired.set_connection_timeout(1);
        const bool absent = !retired.Get("/health", auth);
        const bool destroyed = journal_mobile_qwen_destroy_v1(handle) == JOURNAL_MOBILE_QWEN_OK;
        std::fprintf(stderr, "generation %llu joined retirement=%lldms stopped=%d listener_absent=%d destroyed=%d\n",
                     static_cast<unsigned long long>(generation), static_cast<long long>(retire_ms), stopped, absent, destroyed);
        if (!witnessed_prefill || !stopped || !absent || !destroyed || retire_ms >= 5000) { return 1; }
    }
    return 0;
}
