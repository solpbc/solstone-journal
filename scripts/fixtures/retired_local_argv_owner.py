# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc


def _build_local_llama_cmd(plan, port):
    binary_path = _required_plan_path(plan.binary_path, "binary_path")
    model_path = _required_plan_path(plan.model_path, "model_path")
    context_tokens = _required_plan_int(plan.context_tokens, "context_tokens")
    parallel_slots = _required_plan_int(plan.parallel_slots, "parallel_slots")
    prompt_cache_mib = _required_plan_int(plan.prompt_cache_mib, "prompt_cache_mib")
    launched_context_tokens = context_tokens * parallel_slots
    device_flag = "CUDA0" if plan.backend == "cuda" else "Vulkan0"
    cmd = [
        str(binary_path),
        "-m",
        str(model_path),
        "--alias",
        plan.model_id,
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--jinja",
        "--n-gpu-layers",
        "999",
        "-c",
        str(launched_context_tokens),
        "--parallel",
        str(parallel_slots),
        "--kv-unified",
        "--cache-ram",
        str(prompt_cache_mib),
        "--no-context-shift",
        "--device",
        device_flag,
    ]
    if plan.mmproj_path is not None:
        cmd.extend(["--mmproj", str(plan.mmproj_path)])
    if "0.0.0.0" in cmd:
        raise RuntimeError("Local server may not bind 0.0.0.0.")
    return cmd
