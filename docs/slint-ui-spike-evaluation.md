# Echolet Slint Cross-Platform Desktop UI Spike (PROJECT-041 / J11.1c)

## Executive Summary

This spike evaluates **Slint 1.18.1** as the single shared desktop UI renderer across Windows, macOS, and Linux for Echolet. The spike builds on the canonical J11.1b shared Control Surface Core (`src/ui/control_surface.rs`), introducing a dedicated, opt-in desktop panel adapter and component tree (`ui/desktop/EcholetPanel.slint`).

Production UI behavior remains completely unchanged by default; the spike is guarded by the Cargo feature `slint-ui-spike` and packaged into a standalone binary `echolet-ui-spike` excluded from production releases.

---

## 1. Architecture & Dependency Direction

The spike strictly adheres to the one-way dependency architecture:

```text
Echolet domain / core
    ->
src/ui/control_surface.rs (ControlSurfaceState / SurfaceAction)
    ->
src/ui/desktop/slint_spike.rs (DesktopPanelViewModel / SlintControlSurfaceAdapter)
    ->
ui/desktop/EcholetPanel.slint (Single shared Slint component tree)
    ->
Slint winit backend (software rendering)
```

- **No renderer-owned product logic**: `EcholetPanel` and the Slint adapter perform zero inference on model state, grouping, or primary actions. All action labels (`Select`, `Download`, `Retry`, `Selected`) and actionability are projected directly from canonical `ModelPresentation.primary_action` and `enabled` flags.
- **Single Component Tree Guard**: Exactly one `.slint` file exists in the repository (`ui/desktop/EcholetPanel.slint`). Accidental platform forks are barred by deterministic unit tests verifying that no `.slint` files exist in `src/platform/` and all platforms share the exact same generated module.

---

## 2. Fixed Panel Dimensions & Layout Mechanics

- **Logical Width**: Exactly **380 px** (`width: 380px`). Fixed, content-independent, with all text labels employing `overflow: elide` to prevent content-driven expansion.
- **Logical Height**: Exactly **240 px** (`height: 240px`). Sized to comfortably house the header (app title and canonical runtime state), model section (capability group header and model rows), diagnostic action control, and footer.
- **Window Properties**: `no-frame: true`, borderless utility panel styling with neutral dark theme (`#18181b` surface, `#27272a` cards, `#3f3f46` borders).

---

## 3. Window & Popup Behavior Findings

| Behavior | Tested Outcome | Implementation Note |
|---|---|---|
| **Fixed Width** | PASSED (380 px) | Invariant maintained regardless of text length or model count |
| **Internal Clicks** | PASSED | Clicks on diagnostic button or model rows fire callbacks without closing window |
| **Escape Key Dismiss** | PASSED | Slint `FocusScope` captures `Key.Escape`, triggering `close_requested` callback to hide window and exit event loop |
| **Focus Loss / Outside Click** | Limitation Documented | Pure Slint alone does not provide a universal "popup/popover" dismissal hook on all platforms without thin host integration. On macOS, an event monitor (`NSEvent.addGlobalMonitorForEventsMatchingMask`) or `NSWindowDelegate.windowDidResignKey` is required. On Windows, `WM_KILLFOCUS` is standard. On Linux X11, `FocusOut` or passive grab is standard; on native Wayland, Layer Shell (`zwlr_layer_shell_v1`) popup semantics are required. |
| **Repeated Reopen** | PASSED | Sub-millisecond latency (~1.3 µs) once initialized; memory overhead stable |

---

## 4. Performance & Size Measurements (macOS Apple Silicon Host)

Measured on macOS Darwin 24.3.0 (arm64 Apple Silicon) using `scripts/benchmark-slint-spike.sh`:

### A. Build Artifact Size (Release Profile)
- Normal Echolet production binary (`target/release/echolet`): **4,318,400 bytes** (4.12 MiB)
- Slint Spike binary (`target/release/echolet-ui-spike`): **12,445,984 bytes** (11.87 MiB)
- Incremental footprint attributable to Slint + winit + software renderer: **8,127,584 bytes** (~7.75 MiB)

### B. Process Memory (Resident Set Size via Mach Task Info)
- Initial process RSS (pre-show): **12.88 MiB** (13,508,608 bytes)
- Visible window RSS (first open): **14.57 MiB** (15,282,176 bytes)
- Steady-state visible RSS (after 2–4s stable event loop): **36.19 – 37.48 MiB**
- Hidden window RSS (after repeated open/hide): **34.84 – 36.13 MiB**

### C. Latency (Release Profile)
- Component creation (`EcholetPanel::new()`): **~138 – 158 ms**
- ViewModel binding: **~20 – 40 µs**
- First window visible proxy: **~10.55 ms**
- Repeated open latency (10 iterations):
  - **Median**: `1.287 µs`
  - **Min**: `1.240 µs`
  - **Max**: `10.555 ms`
- Repeated hide latency (10 iterations):
  - **Median**: `3.521 µs`
  - **Min**: `3.463 µs`
  - **Max**: `18.047 µs`

### D. CPU Utilization
- Idle CPU while panel is visible and settled: **0.0%** (verified via `ps -p <PID> -o %cpu`)

---

## 5. Slint Licensing & Distribution Analysis

### Upstream Terms
- **Exact Version**: `slint = "1.18.1"`, `slint-build = "1.18.1"`
- **Upstream License**: Triple-licensed under:
  1. `GPL-3.0-only` (GNU General Public License version 3)
  2. `LicenseRef-Slint-Royalty-free-2.0` (Slint Royalty-Free License 2.0)
  3. `LicenseRef-Slint-Software-3.0` (Slint Commercial License 3.0)
- **Authoritative References**:
  - Upstream License File: https://github.com/slint-ui/slint/blob/master/LICENSE.md
  - Upstream Licensing Guide: https://slint.dev/legal/licenses

### Implications for Echolet
1. **Echolet Open-Source Distribution**:
   - Echolet is licensed under **Apache-2.0**.
   - If Echolet ships pre-compiled desktop binaries linking Slint under GPLv3, the copyleft terms of GPLv3 would apply to the distributed binary artifact.
   - Alternatively, Slint's **Royalty-Free License 2.0** allows distributing compiled binaries without imposing GPLv3 on the rest of the application codebase, provided the attribution obligation is fulfilled.
2. **Attribution & Runtime Obligations**:
   - Under the Royalty-Free License 2.0, Echolet must include the Slint logo and notice in the product's documentation or About screen/acknowledgments.
   - The royalty-free tier excludes embedded/cloud-server use exceeding threshold limits and competing UI toolkit development, neither of which applies to Echolet.
3. **Future Commercial / Proprietary Considerations**:
   - If Echolet or a downstream fork distributes a closed-source product and wishes to avoid attribution requirements, a paid commercial license from SixtyFPS GmbH (Slint Commercial License 3.0) would be required.

---

## 6. Blockers & Host Work Required Before J11.1d

1. **Tray-Popover Anchoring**:
   - Slint provides `window().set_position(...)` using logical/physical screen coordinates.
   - To anchor the panel below or above the native tray icon:
     - On **macOS**: query the `NSStatusItem` button window frame (`status_item.button().window().frame()`).
     - On **Windows**: query tray icon notification area rect (`Shell_NotifyIconGetRect`).
     - On **Linux**: query `ksni` / DBus StatusNotifierItem geometry or fallback to top-right screen bounds.
2. **Outside-Click / Focus-Lost Dismissal**:
   - Slint needs a small native event hook or OS window message listener to automatically hide the panel when the user clicks elsewhere on the desktop.
3. **Linux Wayland Native Shell**:
   - On native Wayland compositors (without XWayland), absolute window positioning is disallowed by standard Wayland protocols. A Layer Shell surface (`wlr-layer-shell`) or XWayland fallback is needed for utility positioning.
