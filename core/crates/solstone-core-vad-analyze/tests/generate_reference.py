# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regenerate independent, offline VAD test data; never used by the product.

Requires Python 3.11, numpy 1.26.4, kaldi-native-fbank 1.15, kaldiio 2.18.0,
setuptools<81 and onnxruntime 1.25.0. Run from any directory:
python3 core/crates/solstone-core-vad-analyze/tests/generate_reference.py
"""

import hashlib
import json
from pathlib import Path

import kaldi_native_fbank as knf
import kaldiio
import numpy as np
import onnxruntime as ort


ROOT = Path(__file__).resolve().parents[4]
FIXTURES = ROOT / "core/fixtures"
ASSETS = ROOT / "core/models/assets"


def generated_audio(recipe, seed):
    state = recipe["seed"]
    values = np.empty(recipe["samples"], dtype=np.float32)
    for i in range(len(values)):
        state = (1664525 * state + 1013904223) & 0xFFFFFFFF
        noise = np.float32(((state >> 16) & 8191) - 4096) / np.float32(32768)
        if recipe["kind"] == "mixed_seed_lcg":
            scale = np.float32((i // 4000) % 17 + 1) / np.float32(18)
            values[i] = seed[i % len(seed)] * scale + noise / np.float32(8)
        else:
            values[i] = noise
    return values


def timestamps(probs, samples, threshold=0.3, silence_ms=1000, pad_ms=400):
    """Independent sample-domain policy reference, including shared gap padding."""
    spans = []
    start = None
    silence_start = None
    for index, probability in enumerate(probs):
        position = index * 160
        if probability >= threshold:
            silence_start = None
            if start is None:
                start = position
        elif probability < max(threshold - 0.15, 0.01) and start is not None:
            if silence_start is None:
                silence_start = position
            if position - silence_start >= silence_ms * 16:
                if silence_start > start:
                    spans.append([start, silence_start])
                start = None
                silence_start = None
    if start is not None and samples > start:
        spans.append([start, samples])
    if not spans:
        return []
    padding = pad_ms * 16
    spans[0][0] = max(0, spans[0][0] - padding)
    for index, span in enumerate(spans):
        if index + 1 == len(spans):
            span[1] = min(samples, span[1] + padding)
        else:
            following = spans[index + 1]
            gap = following[0] - span[1]
            if gap < 2 * padding:
                boundary = span[1] + gap // 2
                span[1] = boundary
                following[0] = boundary
            else:
                span[1] += padding
                following[0] -= padding
    return spans


def main():
    model = ASSETS / "firered_vad.onnx"
    cmvn = ASSETS / "firered_vad_cmvn.ark"
    seed_bytes = (FIXTURES / "vad_speech_seed.f32le").read_bytes()
    seed = np.frombuffer(seed_bytes, dtype="<f4")
    stats = kaldiio.load_mat(str(cmvn))
    means = stats[0, :-1] / stats[0, -1]
    variance = stats[1, :-1] / stats[0, -1] - means * means
    inverse_std = 1.0 / np.sqrt(np.maximum(variance, 1e-20))
    options = ort.SessionOptions()
    options.intra_op_num_threads = 1
    options.inter_op_num_threads = 1
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    options.enable_cpu_mem_arena = False
    session = ort.InferenceSession(str(model), options, providers=["CPUExecutionProvider"])
    fbank_options = knf.FbankOptions()
    fbank_options.frame_opts.samp_freq = 16000
    fbank_options.frame_opts.frame_length_ms = 25
    fbank_options.frame_opts.frame_shift_ms = 10
    fbank_options.frame_opts.dither = 0
    fbank_options.frame_opts.snip_edges = True
    fbank_options.mel_opts.num_bins = 80
    payload = bytearray()

    def store_bytes(data):
        while len(payload) % 4:
            payload.append(0)
        location = {"byte_offset": len(payload), "byte_length": len(data)}
        payload.extend(data)
        return location

    def store(values):
        return store_bytes(np.asarray(values, dtype="<f4").tobytes())

    inputs = []
    for samples in [1, 399, 400, 559, 560, 16000]:
        recipe = {"kind": "lcg_pcm", "samples": samples, "seed": 941083}
        inputs.append((f"noise_{samples}", generated_audio(recipe, seed), None))
    inputs.extend([
        ("silence", np.zeros(16000, np.float32), None),
        ("speech_seed", seed.copy(), None),
        ("attenuated_seed_45db", seed * np.float32(10 ** (-45 / 20)), None),
        ("attenuated_seed_60db", seed * np.float32(0.001), None),
        ("saturation_and_half_lsb", np.resize(np.array(
            [-40000, -32768, -2.5, -1.5, -0.5, 0.5, 1.5, 2.5, 32768],
            dtype=np.float32) / np.float32(32768), 720), None),
    ])
    for frames in [29999, 30000, 30001, 32098]:
        recipe = {"kind": "mixed_seed_lcg", "samples": 400 + (frames - 1) * 160,
                  "seed": 9871643}
        inputs.append((f"nonperiodic_speech_{frames}_frames",
                       generated_audio(recipe, seed), recipe))

    cases = []
    for name, audio, recipe in inputs:
        pcm = np.clip(np.rint(audio * np.float32(32768)), -32768, 32767).astype(np.int16)
        fbank = knf.OnlineFbank(fbank_options)
        fbank.accept_waveform(16000, pcm.tolist())
        raw = np.array([fbank.get_frame(i) for i in range(fbank.num_frames_ready)],
                       dtype=np.float32).reshape(-1, 80)
        features = ((raw - means) * inverse_std).astype(np.float32)
        probabilities = (session.run(["probs"], {"feat": features[None]})[0].reshape(-1)
                         if len(features) else np.array([], np.float32))
        selected = (sorted({0, 151, 152, 159, 160, len(features) - 1,
                            *range(29838, min(30170, len(features)))})
                    if recipe else list(range(len(features))))
        case = {"name": name, "samples": len(audio), "frames": len(features),
                "long": recipe is not None,
                "audio": {"recipe": recipe} if recipe else {"stored": store(audio)},
                "feature_rows": selected, "features": store(features[selected]),
                "probabilities": store(probabilities),
                "speech": timestamps(probabilities.tolist(), len(audio))}
        if recipe:
            independent = np.concatenate([
                session.run(["probs"], {"feat": features[first:first + 30000][None]})[0].reshape(-1)
                for first in range(0, len(features), 30000)
            ])
            case["no_context_max_probability_difference"] = float(np.abs(independent - probabilities).max())
        cases.append(case)
        print(name, len(features), flush=True)

    metadata = {
        "schema": "firered-vad-reference-v1",
        "generator": "core/crates/solstone-core-vad-analyze/tests/generate_reference.py",
        "reference": "https://github.com/FireRedTeam/FireRedVAD/tree/c30ec49e8cc69642b0ee65362eba11b9d11c6e54",
        "method": "canonical kaldi-native-fbank, float64 original CMVN, whole-recording CPU ONNX posteriors; independent sample-domain span policy",
        "dependencies": {"numpy": np.__version__, "onnxruntime": ort.__version__,
                         "kaldi-native-fbank": "1.15", "kaldiio": "2.18.0"},
        "model_sha256": hashlib.sha256(model.read_bytes()).hexdigest(),
        "cmvn_sha256": hashlib.sha256(cmvn.read_bytes()).hexdigest(),
        "speech_seed_sha256": hashlib.sha256(seed_bytes).hexdigest(),
        "payload_sha256": hashlib.sha256(payload).hexdigest(),
        "cases": cases,
    }
    (FIXTURES / "firered_vad_reference.bin").write_bytes(payload)
    (FIXTURES / "firered_vad_reference.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print("payload bytes", len(payload), flush=True)


if __name__ == "__main__":
    main()
