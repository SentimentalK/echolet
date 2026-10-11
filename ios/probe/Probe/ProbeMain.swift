import UIKit

// Echolet iOS native ASR bridge probe (DEBUG infrastructure, not product UI).
// Calls the Rust `echolet_ios` static bridge: version marker, error-path
// recognizer lifecycle, then — when the pinned bilingual-zh-en model folder is
// bundled — a REAL offline streaming recognition of the pinned official test
// wav through the actual sherpa-onnx C API. No fabricated output; whatever the
// real recognizer reports is printed verbatim.

final class ProbeAppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    // Swift-side declarations of the Rust bridge FFI; only resolvable when the
    // opt-in static binaries are linked via ios/probe/project.yml.
    // `@_silgen_name` binds directly to the C symbols exported by the Rust
    // staticlib; without the linked archive the app must not be built.
    @_silgen_name("echolet_ios_probe_version")
    private func echolet_ios_probe_version() -> UnsafePointer<CChar>?
    @_silgen_name("echolet_ios_recognizer_create")
    private func echolet_ios_recognizer_create(_ modelDir: UnsafePointer<CChar>?) -> UnsafeMutableRawPointer?
    @_silgen_name("echolet_ios_recognizer_destroy")
    private func echolet_ios_recognizer_destroy(_ rec: UnsafeMutableRawPointer?)
    @_silgen_name("echolet_ios_stream_create")
    private func echolet_ios_stream_create(_ rec: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer?
    @_silgen_name("echolet_ios_stream_feed")
    private func echolet_ios_stream_feed(
        _ stream: UnsafeMutableRawPointer?, _ sampleRate: Int32,
        _ samples: UnsafePointer<Float>?, _ numSamples: Int32
    ) -> Int32
    @_silgen_name("echolet_ios_stream_read")
    private func echolet_ios_stream_read(
        _ stream: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<CChar>?, _ outLen: Int32
    ) -> Int32
    @_silgen_name("echolet_ios_stream_destroy")
    private func echolet_ios_stream_destroy(_ stream: UnsafeMutableRawPointer?)

    /// Minimal RIFF/16-bit-PCM/mono WAV reader (no framework dependency).
    static func read16BitPcmMonoWav(path: String) -> [Float]? {
        guard let data = FileManager.default.contents(atPath: path),
              data.count >= 44,
              data.starts(with: Array("RIFF".utf8)),
              data[8..<12].elementsEqual(Array("WAVE".utf8)) else { return nil }
        var pos = 12
        var pcmOffset: Int?
        var pcmLength = 0
        var blockAlign = 0
        while pos + 8 <= data.count {
            let chunkId = String(bytes: data[pos..<pos+4], encoding: .ascii)
            let chunkSize = Int(data[pos+4]) | Int(data[pos+5]) << 8
                | Int(data[pos+6]) << 16 | Int(data[pos+7]) << 24
            if chunkId == "fmt " {
                let audioFmt = Int(data[pos+8]) | Int(data[pos+9]) << 8
                let channels = Int(data[pos+10]) | Int(data[pos+11]) << 8
                blockAlign = Int(data[pos+16]) | Int(data[pos+17]) << 8
                let bits = Int(data[pos+22]) | Int(data[pos+23]) << 8
                guard audioFmt == 1, channels == 1, bits == 16 else { return nil }
            } else if chunkId == "data" {
                pcmOffset = pos + 8
                pcmLength = min(chunkSize, data.count - pos - 8)
            }
            pos += 8 + chunkSize + (chunkSize % 2)
        }
        guard let pcmOffset, blockAlign > 0,
              pcmLength > 0, data.count >= pcmOffset + pcmLength else { return nil }
        let count = pcmLength / 2
        var out = [Float](repeating: 0, count: count)
        let scale: Float = 1.0 / 32768.0
        for i in 0..<count {
            let lo = data[pcmOffset + 2 * i]
            let hi = data[pcmOffset + 2 * i + 1]
            let v = Int16(bitPattern: UInt16(lo) | (UInt16(hi) << 8))
            out[i] = Float(Int(v)) * scale
        }
        return out
    }

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        var lines: [String] = []
        lines.append("ECHOLET_IOS_PROBE_BEGIN")

        let version = echolet_ios_probe_version()
        lines.append(
            "version=\(version.map { String(cString: $0) } ?? "NULL_SYMBOL_MISSING")")

        // Real C-API error path: invalid path must return a NULL handle without
        // crashing the process.
        let badPath = "/nonexistent/echolet-probe-model"
        let badRec = echolet_ios_recognizer_create((badPath as NSString).utf8String)
        lines.append(
            "recognizer_create_invalid_path=\(badRec == nil ? "NULL_OK" : "UNEXPECTED_HANDLE")")
        echolet_ios_recognizer_destroy(badRec)

        // ---- Real offline model inference (opt-in, bundled model folder) --
        guard let modelResPath = Bundle.main.path(
            forResource: "bilingual-zh-en", ofType: nil, inDirectory: nil
        ), let recReal = echolet_ios_recognizer_create(
            (modelResPath as NSString).utf8String
        ) else {
            lines.append(Bundle.main.path(
                forResource: "bilingual-zh-en", ofType: nil, inDirectory: nil) == nil
                ? "real_model=BUNDLED_MODEL_MISSING"
                : "real_model=RECOGNIZER_CREATE_FAILED")
            report(lines)
            return true
        }

        lines.append("real_model=LOADED")
        lines.append("real_model_path=\(modelResPath)")

        if let stream = echolet_ios_stream_create(recReal) {
            if let wavPath = Bundle.main.path(
                forResource: "0.wav", ofType: nil, inDirectory: "bilingual-zh-en/test_wavs"
            ), let samples = Self.read16BitPcmMonoWav(path: wavPath) {
                var totalOk = 0
                samples.withUnsafeBufferPointer { buf in
                    var offset = 0
                    let chunk = 800 // 50 ms @ 16 kHz
                    while offset < samples.count {
                        let n = min(chunk, samples.count - offset)
                        let rc = echolet_ios_stream_feed(
                            stream, 16000, buf.baseAddress! + offset, Int32(n))
                        if rc == 0 { totalOk += n }
                        offset += n
                    }
                }
                lines.append("feed_samples_approved=\(totalOk)/\(samples.count)")
                var out = [CChar](repeating: 0, count: 4096)
                let rclen = echolet_ios_stream_read(stream, &out, 4096)
                if rclen >= 0 {
                    let text = String(cString: out)
                    lines.append("read_rc=\(rclen) transcript_bytes=\(text.utf8.count)")
                    lines.append("transcript=\(text)")
                } else {
                    lines.append("read_rc=\(rclen) (buffer too small, needed \(-rclen))")
                }
            } else {
                lines.append("real_model_wav=MISSING_OR_UNPARSABLE")
            }
            echolet_ios_stream_destroy(stream)
        } else {
            lines.append("real_model_stream=NULL")
        }
        echolet_ios_recognizer_destroy(recReal)

        report(lines)
        return true
    }

    private func report(_ lines: [String]) {
        var lines = lines
        lines.append("ECHOLET_IOS_PROBE_END")
        let text = lines.joined(separator: "\n")
        print(text)
        NSLog(text)

        // Surface in-app for manual confirmation without an attached console.
        let label = UILabel()
        label.numberOfLines = 0
        label.font = .systemFont(ofSize: 10, weight: .semibold)
        label.text = text
        let win = UIWindow(frame: UIScreen.main.bounds)
        let vc = UIViewController()
        vc.view = {
            let v = UIView()
            v.addSubview(label)
            label.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: v.layoutMarginsGuide.leadingAnchor),
                label.trailingAnchor.constraint(equalTo: v.layoutMarginsGuide.trailingAnchor),
                label.topAnchor.constraint(equalTo: v.safeAreaLayoutGuide.topAnchor),
            ])
            return v
        }()
        window = win
        win.rootViewController = vc
        win.makeKeyAndVisible()
    }
}

@main
final class ProbeMain: NSObject {
    static func main() {
        // iOS application entry without Info.plist scene manifest.
        UIApplicationMain(
            CommandLine.argc, CommandLine.unsafeArgv,
            nil, NSStringFromClass(ProbeAppDelegate.self)
        )
    }
}
