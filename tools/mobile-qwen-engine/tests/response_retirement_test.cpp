// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "journal_mobile_qwen.h"
#include "server-embedded.h"
#include "server-queue.h"
#include "httplib.h"
#include "nlohmann/json.hpp"

#include <array>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <functional>
#include <mutex>
#include <thread>
#include <type_traits>

using namespace std::chrono_literals;
using fixture_json = nlohmann::json;

// Linked only into this executable, never the eight-export dynamic image.
bool journal_mobile_qwen_test_silent_loop_exit(journal_mobile_qwen_handle *,
    std::function<void(const server_embedded_progress &)>);

[[noreturn]] static void fail(const char * message) {
    std::fprintf(stderr, "response retirement control failed: %s\n", message);
    std::fflush(stderr);
    std::_Exit(1);
}

template<class T, class = void> struct has_cancel_waiters : std::false_type {};
template<class T> struct has_cancel_waiters<T,
    std::void_t<decltype(std::declval<T &>().cancel_waiters())>> : std::true_type {};

template<class Response> static void waiter_controls() {
    if constexpr (has_cancel_waiters<Response>::value) {
        Response response;
        response.add_waiting_task_id(11);
        server_task_result_ptr waiting;
        std::thread reader([&] { waiting = response.recv(11); });
        response.cancel_waiters();
        reader.join();
        auto cancelled = [](const server_task_result_ptr & result) {
            return result && result->is_error()
                && result->to_json().at("type") == "unavailable_error"
                && result->to_json().at("message") == "local model runtime is stopping";
        };
        if (!cancelled(waiting)) { fail("current waiter did not receive typed cancellation"); }
        response.remove_waiting_task_id(11);
        response.cancel_waiters();
        response.add_waiting_task_ids({12, 13});
        if (!cancelled(response.recv(12)) || !cancelled(response.recv(13))) {
            fail("future waiters did not receive the latched cancellation");
        }
        response.remove_waiting_task_ids({12, 13});

        Response ordinary;
        ordinary.add_waiting_task_id(21);
        auto result = std::make_unique<server_task_result_error>();
        result->id = 21;
        result->err_type = ERROR_TYPE_INVALID_REQUEST;
        result->err_msg = "ordinary control";
        ordinary.send(std::move(result));
        const auto received = ordinary.recv(21);
        if (!received || received->to_json().at("message") != "ordinary control") {
            fail("uncancelled response behavior changed");
        }
        std::fprintf(stderr, "current/future waiter latch and uncancelled control passed\n");
    } else {
        std::fprintf(stderr, "baseline has no owned response cancellation latch\n");
    }
}

int main() {
    const char * model = std::getenv("JOURNAL_MOBILE_QWEN_TEST_MODEL");
    const char * projector = std::getenv("JOURNAL_MOBILE_QWEN_TEST_PROJECTOR");
    if (!model || !projector) { return 77; }
    waiter_controls<server_response>();
    const char * token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const httplib::Headers auth{{"Authorization", std::string("Bearer ") + token}};
    for (uint64_t generation = 1; generation <= 2; ++generation) {
        journal_mobile_qwen_config_v1 config{1, sizeof(config), generation,
            journal_mobile_qwen_runtime_identity(), journal_mobile_qwen_source_revision(),
            "verified-integration-fixture", model, projector, token};
        journal_mobile_qwen_handle * handle = nullptr;
        char error[512]{};
        if (journal_mobile_qwen_create_v1(&config, &handle, error, sizeof(error)) != JOURNAL_MOBILE_QWEN_OK) {
            fail("create");
        }
        std::mutex progress_mutex;
        std::condition_variable progress_changed;
        bool observe_active = false;
        bool witnessed_decode = false;
        int active_task = -1;
        bool silent_loop_exit = false;
        if (!journal_mobile_qwen_test_silent_loop_exit(handle, [&](const server_embedded_progress & value) {
            std::lock_guard<std::mutex> lock(progress_mutex);
            if (!observe_active) { return; }
            if (!value.terminal && value.phase == server_embedded_phase::decode
                && value.prefill_complete && value.generated > 0 && value.generated < value.output_limit) {
                witnessed_decode = true;
                active_task = value.task;
                progress_changed.notify_all();
            }
            if (value.terminal && value.task == active_task
                && value.phase == server_embedded_phase::loop_exit) {
                silent_loop_exit = true;
            }
        })) { fail("private silent-loop fixture"); }
        int engine_result = -1;
        std::thread engine([&] { engine_result = journal_mobile_qwen_run_v1(handle); });
        journal_mobile_qwen_snapshot_v1 snapshot{1, sizeof(snapshot)};
        const auto ready_deadline = std::chrono::steady_clock::now() + 120s;
        do {
            journal_mobile_qwen_get_snapshot_v1(handle, &snapshot);
            if (snapshot.state == JOURNAL_MOBILE_QWEN_READY || snapshot.state == JOURNAL_MOBILE_QWEN_FAILED) { break; }
            std::this_thread::sleep_for(20ms);
        } while (std::chrono::steady_clock::now() < ready_deadline);
        if (snapshot.state != JOURNAL_MOBILE_QWEN_READY) { fail("ready"); }
        const auto port = snapshot.bound_port;
        httplib::Client regular("127.0.0.1", port);
        regular.set_read_timeout(90);
        const auto completion = regular.Post("/completion", auth,
            R"({"prompt":"Say hello.","n_predict":8,"cache_prompt":false})", "application/json");
        if (!completion || completion->status != 200
            || fixture_json::parse(completion->body).at("content").get<std::string>().empty()) {
            fail("ordinary completion before Stop");
        }
        {
            std::lock_guard<std::mutex> lock(progress_mutex);
            observe_active = true;
        }
        const auto body = fixture_json{{"prompt", "Continue counting consecutive numbers, starting at one."},
            {"n_predict", 4096}, {"ignore_eos", true}, {"cache_prompt", false}}.dump();
        std::array<int, 2> statuses{{-1, -1}};
        std::array<std::string, 2> bodies;
        std::array<std::thread, 2> clients;
        for (size_t index = 0; index < clients.size(); ++index) {
            clients[index] = std::thread([&, index] {
                httplib::Client client("127.0.0.1", port);
                client.set_read_timeout(120);
                const auto response = client.Post("/completion", auth, body, "application/json");
                if (response) { statuses[index] = response->status; bodies[index] = response->body; }
            });
        }
        {
            std::unique_lock<std::mutex> lock(progress_mutex);
            if (!progress_changed.wait_for(lock, 90s, [&] { return witnessed_decode; })) {
                fail("no active decode before Stop");
            }
        }
        std::fprintf(stderr, "generation %llu actual decode witnessed; CPU abort disabled, live clients retained\n",
                     static_cast<unsigned long long>(generation));
        std::mutex joined_mutex;
        std::condition_variable joined_changed;
        bool joined = false;
        std::thread watchdog([&] {
            std::unique_lock<std::mutex> lock(joined_mutex);
            if (!joined_changed.wait_for(lock, 8s, [&] { return joined; })) {
                fail("silent model-loop exit did not join within8s while HTTP clients stayed connected");
            }
        });
        const auto stop_started = std::chrono::steady_clock::now();
        journal_mobile_qwen_request_stop_v1(handle);
        engine.join();
        for (auto & client : clients) { client.join(); }
        const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::steady_clock::now() - stop_started).count();
        {
            std::lock_guard<std::mutex> lock(joined_mutex);
            joined = true;
            joined_changed.notify_all();
        }
        watchdog.join();
        journal_mobile_qwen_get_snapshot_v1(handle, &snapshot);
        if (engine_result != JOURNAL_MOBILE_QWEN_OK || snapshot.state != JOURNAL_MOBILE_QWEN_STOPPED
            || snapshot.bound_port != 0 || !silent_loop_exit) { fail("quiet loop/engine retirement state"); }
        for (size_t index = 0; index < statuses.size(); ++index) {
            if (statuses[index] != 503 || fixture_json::parse(bodies[index]).at("error").at("type") != "unavailable_error") {
                fail("live client did not receive normal typed cancellation");
            }
        }
        httplib::Client retired("127.0.0.1", port);
        retired.set_connection_timeout(1);
        if (retired.Get("/health", auth)) { fail("retired listener remains"); }
        if (journal_mobile_qwen_destroy_v1(handle) != JOURNAL_MOBILE_QWEN_OK) { fail("destroy after real join"); }
        std::fprintf(stderr, "generation %llu silent loop joined/destroyed in%lldms;2live clients503; listener absent\n",
                     static_cast<unsigned long long>(generation), static_cast<long long>(elapsed));
    }
    return 0;
}
