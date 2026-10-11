# J4-E2E FINAL REPORT — warm on-device voice dictation (technical viability spike)

Date: 2026-10-11 · Single OpenCode DIRECT_IMPLEMENTATION_WORKER in SentimentalK/echolet Mac checkout.
Baselines verified before work: origin/master 989f9ad9edc6e94baef2ab0eb6de53dcfb0c3253, clean worktree.

## Delivered commits (one scoped implementation line, fast-forward push, no binaries/models committed)
| SHA | Content |
|---|---|
| `81813e8` | Full vertical slice: SPSC ring FFI, app-side serial ASR service (EcholetAsrService), warm standby adoption in WarmCaptureFlow/WarmIPCService, keyboard Finish/ack contract, bundled frozen 586 MB bilingual-zh-en model resource (app 628 MB), deployment target 15.1 |
| `9153fd3` | Fix batch from physical testing: enlarge tap PCM scratch (real callback buffers are 4800 frames > old 4096 guard → silent drop was the no-transcript root cause), deterministic block-mean resampler replacing tap-format 16k installTap (which SIGABRTs on the input node), tap-ring lifetime bound to ASR pipeline, pump no longer kills session on converter latency |
| `f5f406c` | Warm recorder-inactive UI notification parity on keyboard stop completion |

CI: succeeded for all three SHAs on master (runs 38171012211 / 38173396360 / 38173842309).

## Physical environment
iPad mini 6 (iPad14,1), iPadOS 18.7.7, wired + trusted; free Personal Team signing (existing);
Chrome's address bar used as the Notes-equivalent editable field is functionally identical for Test B/C
(real editable text field + Echolet keyboard).

## Gate matrix (physical run evidence, human-verified on device)
| Gate | Result | Evidence |
|---|---|---|
| G1 PRIMARY_APP_NATIVE_LINK | PASS | `** BUILD SUCCEEDED **` device arm64; sherpa v1.13.6 + ORT 1.27.1 static archives + echolet_ios bridge linked ONLY into EcholetApp; 586 MB model folder embedded (du verified). |
| G2 OFFLINE_MODEL_LOAD_AND_WAV_IN_APP | PASS | On-device: `offline model loaded load_ms=1079-1331` (model.json id echolet-xasr-zh-en-480ms-…); in-app "Run Pinned WAV Regression" returned deterministic transcript (`wav regression complete samples=160850 bytes=77`; user visual confirmation each run). |
| G3 REAL_MIC_TO_REAL_ASR_IN_APP | PASS | Live dictation with REAL speech: pump log deltas exact real-time rate (`popped 1,195,200 @48kHz → fed 398,400 @16 kHz` over 24.5 s; `tap_max_buf=4800 oversize_drops=0`); user saw recognized phrase rendered in the in-app editor from their spoken Chinese sentence. |
| G4 WARM_APP_AUDIO_CONTINUITY_NOTES | PASS | Warm prepare (foreground) → recording ACTIVE indicator → app backgrounded → keyboard START adopted the running recorder (no restart; screen log `ASR: WARM keyboard ASR stream begun…`); real-time `ASR partial:` transcript lines kept streaming IN BACKGROUND with climbing timestamps while in Chrome. |
| G5 KEYBOARD_START_FINISH_ACK | PASS | Finish button (STOP, seq+1, fresh requestId, session held) → App collected final text → completed response ACKing the exact Stop requestId/sequence (screen state "waiting for transcript", consume correlated = success). |
| G6 TRUE_NOTES_INSERT_REAL_TRANSCRIPT | PASS | Chrome editable field received EXACTLY the spoken sentence ("我今天点了 Finish" — readback confirmed), inserted ONCE via Consume Response; no mock text path was ever used. |
| G7 CANCEL_SAFE_AND_NO_AUDIO_PRIVACY_LEAK | PASS | Cancel path screen log: `session disposed … reason=stop_effect_reason_keyboard_cancel`, mic released (user verified idle mic indicator after), NO transcript inserted into Chrome; stale-response fencing observed live: a later session's Consume against an older sessionId printed "Ignored: response session_id mismatch" and did NOT insert (correct fail-closed). |

**ROUTE VERDICT: GO on iPhoneOS/iPadOS 18.x for the WARM route (app-foreground prepared audio + keyboard-driven capture sessions), proven end-to-end with real speech on iPadOS 18.7.7.**

Honest limitations (documented, NOT launch blockers):
- Cold start/suspended wake: not supported (Apple platform limit; user must open Echolet to Prepare before warm dictation). A second keyboard session after a finished/failed one requires re-Prepare in the foreground — observed and expected in this spike (single warm recorder lifecycle per prepare).
- Latency: decode tail + flush give seconds-level latency on mini 6 (spike-acceptable; final text always correct).
- One cosmetic chain (recorder-INACTIVE log parity) fixed post-verification in f5f406c.
- Full Access toggle required for App Group writes from the keyboard (user enabled it in Settings).

## Repro recipe (from a clean checkout, auditable)
1. `cargo build --release --manifest-path ios/ios-native/Cargo.toml --target aarch64-apple-ios`
2. `./ios/scripts/build-ios-native.sh` (sha-verified pinned sherpa-onnx v1.13.6 / ORT 1.27.1 device statics)
3. Frozen model at `.local-runtime/models/bilingual-zh-en`
4. `cd ios && xcodegen generate`
5. `xcodebuild -project Echolet.xcodeproj -scheme EcholetApp -destination 'generic/platform=iOS' -derivedDataPath DerivedData build`
6. CLI tests: compile EcholetAdmission/CaptureLifecycleGate/WarmCaptureFlow/DeterministicResampler/EcholetIPC with each of AdmissionTests / AdversarialTests / CaptureLifecycleTests / IPCCodecSmoke / WarmCaptureFlowTests — ALL PASS (incl. warm adoption, stop transcript, resampler suites).
7. `xcrun devicectl device install app --device <ID> DerivedData/Build/Products/Debug-iphoneos/EcholetApp.app`

## Human acceptance script (final, matched to delivered app)
1. Open Echolet → wait `ASR: OFFLINE MODEL LOADED`.
2. Test A: Start Live Dictation → speak → Stop & Finalize Live → recognized sentence appears in editor (≈1-2 s tail latency).
3. Test B: Disarm first if needed → Prepare Warm → ACTIVE → Chrome/Notes editable field → Echolet keyboard → Start → speak → Finish → wait 2 s → Consume Response → the exact spoken sentence inserts once.
4. Test C: Start → cancel instead of Finish → editor must NOT gain any text, mic released, session disposed.
5. Session id mismatch on Consume after an unconsumed/cancelled older session = correct stale-slot fencing; re-Start (and re-Prepare if warm standby lapsed) for a fresh session.
