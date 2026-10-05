# Echolet J9 — Streaming ASR Candidate Matrix (2026)

**PROJECT-041 Stage 3 / J9.** Decision-ready research + reproducible benchmark of
new multilingual / language-expansion streaming ASR packs for Echolet, measured
against the current `Echolet Verified` X-ASR zh/en 480 ms baseline.

- Status: **research + benchmark complete** (no product registry change, no new
  `Echolet Verified` entry, J10 not started).
- Research date: **2026-10-05** (current-model survey).
- Baseline under test: J8 commit `41d2bfcb00ec6bdf34fc44a545b784a1a6fb6197`,
  `echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1`.
- Machine: macOS 14.6.1, **x86_64 Intel Core i7-9750H @ 2.60 GHz**, 32 GiB RAM,
  Rust 1.99.0, bundled **sherpa-onnx v1.13.6** (`libsherpa-onnx-c-api.dylib`).
- Machine-readable pinning: `models/candidates.lock.json` (non-product).
- Reproducible harness: `tests/test_j9_candidate_benchmark.rs` (opt-in; no-op in CI).

> Claims below are explicitly split into **Measured (local)**, **Upstream-reported**,
> and **Judgment**. Local smoke WAVs establish compatibility/latency only; they are
> **not** a global quality ranking.

---

## 1. Executive recommendation

| Decision | Pack | Why |
| --- | --- | --- |
| **J10 (recommended, 1 pack)** | NVIDIA **Nemotron 3.5 ASR Streaming 0.6B / 560 ms / int8** (`sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11`) | One 0.6B pack adds up to **40 language-locales** (32 transcription-ready out of the box incl. zh, ja, ko, es, fr, de, pt, ru, ar, hi, vi, uk), runs natively in the existing sherpa-onnx streaming path, is smaller than the current baseline archive, stays >1.8x realtime on a laptop CPU, and emits punctuation/capitalization. |
| **Keep baseline** | X-ASR zh/en 480 ms | Nothing measured clearly beats it for zh+en: it is faster (RTF 0.19 vs 0.51-0.62), smaller installed, and remains the default. Default answer to "replace X-ASR" is **NO**. |
| **Watch (no J10)** | Chinese Zipformer XLarge int8; Korean / Vietnamese / Bengali Zipformer; streaming Paraformer trilingual zh/yue/en | Useful language coverage, but superseded by Nemotron 3.5 for most, or too large (Paraformer ~1 GB), or niche. Revisit only if a specific language gap is demand-driven. |
| **Reject** | Omnilingual-ASR 1600-lang CTC, Qwen3-ASR 0.6B, Parakeet TDT 0.6B v3, Moonshine, Dolphin, NeMo FastConformer 10-lang, FunASR nano | No true cache-aware streaming / incremental decode path; they are offline or "simulated streaming" and violate Echolet's mandatory low-latency local dictation. |

**J10 blocker to plan for:** the multilingual model's per-stream language hint is
**not reachable** through Echolet's current Rust ASR API. Auto-detect worked for
zh/es/vi but **misdetected Japanese as Korean-ish text** in the smoke test
(`ja.wav`), so J10 must expose `SherpaOnnxOnlineStreamSetOption(stream, "language", "<code>")`
(already present as a C symbol in the bundled v1.13.6 runtime) plus a model/language
metadata + selection surface. This is a small, contained change, not a new stack.

---

## 2. Survey method and hard eligibility filters

Current external research covered: the official `k2-fsa/sherpa-onnx` `asr-models`
release and pretrained-model docs, the NVIDIA NeMo model cards (Nemotron 3 / 3.5),
Meta Omnilingual ASR, and 2026 streaming releases. Candidates were surveyed across
four categories:

1. official sherpa-onnx streaming transducer/multilingual models;
2. strong 2026 streaming ASR releases from major/open projects that can run locally;
3. language-specialized packs that expand Echolet beyond zh/en;
4. already-mentioned/staged experiments in this repo (Nemotron).

Each candidate was filtered by:

| Filter | Outcome |
| --- | --- |
| True streaming / practical incremental decode | Rejected all offline/CTC/LLM/encoder-decoder batch models (Omnilingual, Qwen3-ASR, Parakeet TDT, Moonshine, Dolphin, FastConformer 10-lang, FunASR nano). |
| Fits current local CPU runtime without a new inference stack | Passing candidates all load through the existing `OnlineRecognizer` (NeMo transducer or Zipformer2 transducer). |
| Redistribution license | OpenMDW-1.1 (Nemotron 3.5), NVIDIA Open Model License (Nemotron en), Apache-2.0 (icefall/k2-fsa Zipformer, FunASR Paraformer), Apache-2.0 (Vosk). No non-redistributable finalist. |
| Immutable/reproducible upstream revision | Finalists pinned by release URL + SHA256 in `models/candidates.lock.json`; NVIDIA weights by HF model card; sherpa export by release tag/PR. |
| Architecture requiring large rewrite | None for finalists; multilingual prompt conditioning is the only new (small) integration need. |
| Quality evidence | Upstream WER reported for Nemotron (see section 5); local smoke used only for sanity. |
| Maintained source | Nemotron 3.5 is a 2026 release actively exported by k2-fsa; Zipformer 2025-06-30 is current. |

---

## 3. Candidate matrix

`Recommendation` states: **J10 candidate / Benchmark / Watch / Reject**.
Fields marked `-` were not authoritatively published and were not needed to decide.

| Candidate | Upstream (revision/date) | Langs | Streaming arch / chunk | License | Runtime path | State |
| --- | --- | --- | --- | --- | --- | --- |
| **Nemotron 3.5 ASR Streaming 0.6B int8** `...-560ms-int8-2026-06-11` | `nvidia/nemotron-3.5-asr-streaming-0.6b`; export k2-fsa PR #3671 merge `b74c4df`; pkg 2026-06-11 | 40 locales (32 out-of-the-box; zh, ja, ko, es, fr, de, pt, ru, ar, hi, vi, uk + others) | Cache-aware FastConformer-RNNT, `prompt_index` language conditioning; 80/160/560/1120 ms | OpenMDW-1.1 | Native sherpa-onnx NeMo transducer; per-stream `language` option | **J10 candidate** |
| Nemotron Speech Streaming en 0.6B int8 `...-560ms-int8-2026-04-25` | `nvidia/nemotron-speech-streaming-en-0.6b` | en | Cache-aware FastConformer-RNNT; 80/160/560/1120 ms | NVIDIA Open Model License | Native sherpa-onnx NeMo transducer | Benchmark (en reference) |
| Zipformer zh XLarge int8 `...-zh-xlarge-int8-2025-06-30` | `yuekai/icefall-asr-multi-zh-hans-zipformer-xl` | zh | Streaming Zipformer transducer | Apache-2.0 (icefall/k2-fsa) | Native sherpa-onnx Zipformer2 | Benchmark / Watch |
| Zipformer zh (large) int8 `...-zh-int8-2025-06-30` | `yuekai/icefall-asr-multi-zh-hans-zipformer-large` | zh | Streaming Zipformer transducer | Apache-2.0 | Native Zipformer2 | Watch |
| Zipformer Korean `...-korean-2024-06-16` | k2-fsa icefall | ko | Streaming Zipformer transducer | Apache-2.0 | Native Zipformer2 | Watch |
| Zipformer Vietnamese 30M int8 `...-vi-30M-int8-2026-02-09` | k2-fsa icefall | vi | Streaming Zipformer transducer (30M) | Apache-2.0 | Native Zipformer2 | Watch |
| Zipformer Bengali (Vosk) `...-bn-vosk-2026-02-09` | `alphacep/vosk-model-small-streaming-bn` | bn | Streaming Zipformer transducer | Apache-2.0 | Native Zipformer2 | Watch |
| Streaming Paraformer trilingual zh/yue/en | FunASR Paraformer | zh, yue, en | Streaming Paraformer (NAR) | Apache-2.0 | Native sherpa-onnx Paraformer | Reject/Watch (Cantonese-only gap; ~1 GB) |
| NeMo FastConformer 10 European langs | `nvidia/stt_multilingual_fastconformer_hybrid_large_pc` | be,de,en,es,fr,hr,it,pl,ru,uk | Batch/offline hybrid; no true cache-aware streaming | CC-BY-4.0 | Offline/simulated only | Reject |
| Omnilingual ASR 300M CTC int8 `...-1600-languages-...-2025-11-12` | Meta `facebook/omnilingual-asr` | 1600+ | wav2vec2 CTC, offline | Apache-2.0 | Offline CTC only | Reject (no streaming) |
| Qwen3-ASR 0.6B int8 | Qwen | 52 | LLM offline ASR | Apache-2.0 | Offline / VAD-simulated | Reject (latency) |
| Parakeet TDT 0.6B v3 | `nvidia/parakeet-tdt-0.6b-v3` | 25 EU | FastConformer TDT offline | CC-BY-4.0 | Offline/simulated | Reject |
| Moonshine base int8 | Useful Sensors | en | Encoder-decoder offline | MIT | Offline/simulated | Reject |
| Dolphin base CTC, FunASR nano | various | multi | Offline | varies | Offline | Reject |

Categories intentionally surveyed but with no better streaming fit: `sherpa-onnx-omnilingual`
(offline only), `nemo-fast-conformer-transducer-...-20k` (offline), `qwen3-asr` (offline),
`parakeet_unified`/`parakeet_tdt` sample APKs (offline "simulated streaming").

---

## 4. Benchmarked finalists — measured results

All four models were downloaded from immutable `k2-fsa/sherpa-onnx` `asr-models`
release URLs, archive SHA256 verified, and benchmarked one model per fresh process
on the machine above. Command in section 8.

| Model (int8) | Archive MB | Installed MB | Cold load ms | First partial ms | Decode RTF | RSS after load MiB | Probe (native rate) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| **X-ASR zh/en 480 ms (baseline)** | 551.8 | 614.6 | 3630 | 282 | **0.192** | ~1095 | bilingual 0.wav (16 kHz, 10.1 s) |
| **Nemotron 3.5 multi 560 ms** | 475.3 | 682.2 | 3357 | 339 | 0.542 | ~1016 | zh (24 kHz, 4.8 s) |
| Nemotron en 560 ms | 463.9 | 661.9 | 4323 | 374 | 0.618 | ~1036 | en 0.wav (16 kHz, 6.6 s) |
| Zipformer zh XLarge | 597.8 | 771.2 | 5832 | 316 | 0.506 | ~1192 | zh 0.wav (16 kHz, 5.6 s) |

Additional Nemotron 3.5 probes (same process as its zh row; RSS deltas thereafter
are order-sensitive and therefore omitted):

| Probe | Native rate | First partial ms | RTF | Transcript (local smoke) |
| --- | ---: | ---: | ---: | --- |
| zh | 24 kHz | 339 | 0.542 | `不要问你的国家能为你做什么, 而要问你能为你的国家做什么` (correct) |
| ja | 44.1 kHz | 322 | 0.681 | `근이があなたのために何ができるかを通のではなく、あなたが国のために何ができるかを到ってく` (**auto-detect failed**: Korean-ish; needs forced `language=ja`) |
| es | 22.05 kHz | 663 | 0.556 | `preguntes que puede hacer tu país porti pregunta qué puedes hacer tú por tu pa` (correct) |
| vi | 16 kHz | 328 | 0.567 | `Hỏi đất nước có thể làm gì cho bạn?  Hãy hỏi bạn có thể làm gì cho đất n` (correct) |

Nemotron en probes: `0.wav` -> `After early nightfall, the yellow lamps would light
up here and there the squalid quarter of the brothels`; `1.wav` -> the
`God as a direct consequence ...` passage. Both clean.
Zipformer zh XLarge: `0.wav` -> `对我做了介绍啊那么我想说的是呢大家如果对我的研究感兴趣呢`;
`1.wav` -> `重点呢想谈三个问题首先呢就是这一轮全球金融动荡的表现`. Both clean.

**Caveats (measured columns).** Single x86_64 laptop CPU, 1 decode thread, greedy
search, release build. RTF is normalized but the probe audio differs per row, so
treat cross-model RTF as indicative, not a controlled race. `First partial` is a
200 ms-chunk proxy for streaming latency, not an audio-alignment measurement. RSS
is macOS resident size and allocator-caching dependent; only the first load per
process is clean. No temperature control or repetition statistics were run.

---

## 5. Upstream-reported quality metrics (not local measurements)

From the NVIDIA Nemo model cards, clearly attributed and **not** reproduced here:

- **Nemotron 3.5 ASR Streaming 0.6B** (FLEURS test, 1.12 s frame size, LangID):
  English 7.91, Spanish 4.11, French 9.03, Italian 4.25, Portuguese 5.48,
  German 8.31, Hindi 6.81, Korean 7.12 WER. 40 language-locales total; 32
  transcription-ready / 13 broad-coverage / 8 adaptation-ready (fine-tune).
- **Nemotron Speech Streaming en 0.6B** (1.12 s frame size): AMI IHM 11.73,
  Earnings22 12.52, GigaSpeech 9.66 WER.
- X-ASR has no upstream WER published in this repo; J8 validated identity and a
  real transcript, not aggregate quality.

Judgment: Nemotron 3.5's reported multilingual WER (roughly single-digit on
FLEURS for major languages) is credible for dictation but is measured at the
1.12 s operating point, whereas Echolet would ship the 560 ms chunk size; expect
some quality regression at the lower latency and require a J10 acceptance WER
before promotion.

---

## 6. Runtime / conversion feasibility

- **Nemotron 3.5 & Nemotron en** — load directly in the bundled sherpa-onnx
  v1.13.6 NeMo online-transducer path (verified locally, section 4). No ONNX
  conversion for Echolet. Multilingual needs per-stream language selection:
  `SherpaOnnxOnlineStreamSetOption(stream, "language", "ja")`; the symbol is
  already exported by the bundled dylib (`nm` confirms `_SherpaOnnxOnlineStreamSetOption`),
  but Echolet's `src/ffi.rs` / `src/asr.rs` do not expose it yet. Pack layout:
  `encoder.int8.onnx`, `decoder.int8.onnx`, `joiner.int8.onnx`, `tokens.txt`.
  Tokenizer is already converted to `tokens.txt` at export time.
- **Zipformer zh XLarge** — loads directly as a Zipformer2 transducer; note the
  mixed quantization layout (`encoder.int8.onnx`, `decoder.onnx`,
  `joiner.int8.onnx`), which the registry `files` map already supports. No
  conversion, no runtime change.
- **Windows/macOS/Linux/Android** — all finalists use the same C API already
  shipped across platforms; Android uses the same sherpa-onnx stack. No
  platform-specific conversion path.
- **Quantization choices** — only int8 variants were benchmarked; fp16/fp32 exist
  for Zipformer and HF mirrors exist for Nemotron, but int8 is the right default
  for on-demand-downloaded CPU dictation.

---

## 7. Multilingual strategy recommendation

1. **Move exactly one pack to J10:** Nemotron 3.5 ASR Streaming 0.6B, 560 ms, int8.
   It is the only candidate that simultaneously (a) truly streams, (b) runs on the
   existing runtime, (c) expands one pack to 32 usable locales, (d) is no larger
   than the current baseline, and (e) is permissively licensed (OpenMDW-1.1).
2. **Expose per-stream language selection in J10.** Without it, auto-detect is
   usable for zh/es/vi but demonstrably wrong for Japanese. Add a small FFI
   declaration + `OnlineStream` method + registry/manifest language metadata, and a
   user-facing language control (Auto + explicit codes).
3. **Do not replace X-ASR.** It remains the default: fastest measured RTF (0.19),
   smallest installed footprint, and already `Echolet Verified`. Nemotron 3.5 is an
   additive multilingual pack, not a zh/en upgrade.
4. **No English-only pack in J10.** Nemotron en is slower than X-ASR and adds no
   language coverage; only reconsider if a measurable English-quality gap appears.
5. **Watch, don't ship:** Korean / Vietnamese / Bengali Zipformer packs and
   streaming Paraformer trilingual (the only unique gap is Cantonese) — add only
   if user demand justifies per-language packs over one multilingual model.
6. **Reject offline models for dictation.** Omnilingual-ASR (1600 languages) is
   attractive for coverage but has no incremental decode; keep it as a possible
   future background/batch feature, not streaming dictation.

---

## 8. Reproducibility

Non-product candidate lock: `models/candidates.lock.json` (URL + SHA256 + size +
license + revision per finalist). Archive SHA256:

| Candidate | archive SHA256 |
| --- | --- |
| Nemotron 3.5 multi 560 ms int8 | `c6bf5e0df765f9d5b43bc9e0536d4b4b3e7d40bdf5ecf13e45f134c51c05ae3a` |
| Nemotron en 560 ms int8 | `78e2b79fcf7271553a74402a76b771b09ea40117a39566a79f52235b23db6358` |
| Zipformer zh XLarge int8 | `30437d84dc4861740d40166212ab05f3728945443d53691620075c4031fb88e1` |

Harness (opt-in, no-op in ordinary CI — it never downloads or reads `registry.json`):

```sh
ECHOLET_J9_BENCH=1 \
ECHOLET_J9_MODELS_DIR=/path/to/extracted/candidates \
ECHOLET_J9_OUT=/tmp/j9-bench.json \
cargo test --test test_j9_candidate_benchmark -- --nocapture
```

- `ECHOLET_J9_BASELINE_DIR` overrides the default baseline install dir
  (repo-relative `.local-runtime/models/bilingual-zh-en`).
- `ECHOLET_J9_ONLY=<candidate-id|baseline>` runs one model per process for clean
  RSS deltas.
- The harness reads real WAV headers (16-bit PCM / 32-bit float, mixed native
  sample rates 16/22.05/24/44.1 kHz) and passes the true rate to sherpa.

No weights or audio are committed. The old ad-hoc `tests/test_multi_model_asr.rs`
(which contained developer-machine absolute paths and uncovered Nemotron
experiments) was removed and replaced by this reproducible harness; no candidate
was turned into product behavior.

---

## 9. Primary sources (accessed 2026-10-05)

- k2-fsa/sherpa-onnx `asr-models` release: https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models
- sherpa-onnx pretrained models (Zipformer / NeMo / Omnilingual):
  https://k2-fsa.github.io/sherpa/onnx/pretrained_models/index.html
- Nemotron streaming docs: https://k2-fsa.github.io/sherpa/onnx/nemo/nemotron-streaming.html
- Multilingual Nemotron support PR #3671 (merge `b74c4df`, 2026-06-12):
  https://github.com/k2-fsa/sherpa-onnx/pull/3671
- NVIDIA `nvidia/nemotron-3.5-asr-streaming-0.6b` (OpenMDW-1.1, release 06/04/2026):
  https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b
- NVIDIA `nvidia/nemotron-speech-streaming-en-0.6b`:
  https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b
- Zipformer zh XLarge (icefall `multi_zh-hans`): https://huggingface.co/yuekai/icefall-asr-multi-zh-hans-zipformer-xl
- Omnilingual ASR: https://huggingface.co/facebook/omnilingual-asr
- OpenMDW-1.1 license: https://openmdw.ai/license/1-1/
- NVIDIA Open Model License: https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/

---

## 10. Blockers / unknowns

- **Per-stream language API** is the only real J10 code change (section 1/7);
  multilingual quality for langs other than en/zh/es/vi is upstream-reported, not
  locally reproduced.
- **License diligence:** Zipformer/Vosk/Paraformer are Apache-2.0 via icefall /
  k2-fsa / FunASR; the exact HF model-card license text for
  `yuekai/icefall-asr-multi-zh-hans-zipformer-xl` does not restate it, so J10 must
  confirm the upstream license file before any frozen redistribution.
- **560 ms vs 1.12 s quality:** upstream WER is reported at 1.12 s; the shipped
  560 ms operating point needs a J10 acceptance WER on an in-domain set.
- **Benchmark dialect:** single-machine, single-thread, greedy search only; no
  beam search, no GPU, no repetition statistics. Absolute numbers are
  environment-specific.
- No J10 code, no registry edit, no `Echolet Verified` promotion, and no model
  artifacts were produced by J9.
