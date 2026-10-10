package com.mainstayx.echolet

import android.content.Context
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executor
import java.util.concurrent.Executors

/**
 * Android Model Download and Installation Manager (Phase 1-B).
 *
 * Responsibilities:
 * - Downloads catalog models via standard HTTPS [HttpURLConnection] into temporary `.part` files.
 * - Enforces HTTPS, connect/read timeouts (30s), and streams download progress.
 * - Periodically updates JNI progress via [NativeBridge.nativeSetDownloadProgress].
 * - On download completion, delegates verification and atomic installation to the Rust core
 *   via [NativeBridge.nativeInstallModelFromArchive].
 * - Cleans up temporary staging files on completion or failure.
 * - Protects against multiple concurrent downloads of the same model.
 */
class ModelDownloadManager(
    private val context: Context,
    private val executor: Executor = Executors.newSingleThreadExecutor { r ->
        Thread(r, "echolet-model-dl").apply { isDaemon = true }
    },
    private val onProgressOrStatusChanged: () -> Unit = {},
) {

    private val activeDownloads = ConcurrentHashMap<String, Boolean>()

    fun isDownloading(modelId: String): Boolean = activeDownloads[modelId] == true

    /**
     * Finds model URL from canonical snapshot or catalog metadata.
     */
    private fun findModelUrl(modelId: String): String? {
        val json = try {
            NativeBridge.nativeModelSnapshot()
        } catch (t: Throwable) {
            Log.e(TAG, "Failed to get model snapshot", t)
            return null
        }
        // In our canonical catalog, we can lookup the URL by known model IDs
        // or parse the URL from registry if exposed.
        return when (modelId) {
            "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1" ->
                "https://github.com/SentimentalK/echolet/releases/download/model-xasr-zh-en-480ms-r1/model-xasr-zh-en-480ms-r1.tar.zst"
            "echolet-kroko-streaming-en-2025-08-06-r1" ->
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06.tar.bz2"
            "echolet-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25-r1" ->
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25.tar.bz2"
            "echolet-parakeet-unified-en-0.6b-560ms-int8-2026-05-12-r1" ->
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-560ms.tar.bz2"
            else -> null
        }
    }

    /**
     * Initiates background download and install of a catalog model.
     */
    fun startDownload(modelId: String) {
        if (activeDownloads.putIfAbsent(modelId, true) != null) {
            Log.w(TAG, "Download already active for model: $modelId")
            return
        }

        val urlString = findModelUrl(modelId)
        if (urlString == null) {
            Log.e(TAG, "Unknown model ID for download: $modelId")
            activeDownloads.remove(modelId)
            NativeBridge.nativeSetDownloadProgress(modelId, 0, 0, "failed")
            onProgressOrStatusChanged()
            return
        }

        executor.execute {
            var tempPartFile: File? = null
            try {
                NativeBridge.nativeSetDownloadProgress(modelId, 0, 0, "starting")
                onProgressOrStatusChanged()

                val url = URL(urlString)
                if (!url.protocol.equals("https", ignoreCase = true)) {
                    throw IllegalArgumentException("Only HTTPS downloads are supported")
                }

                val cacheDir = context.cacheDir ?: context.filesDir
                val ext = if (urlString.endsWith(".tar.zst")) ".tar.zst" else ".tar.bz2"
                tempPartFile = File(cacheDir, "echolet-dl-$modelId-$ext.part")
                if (tempPartFile.exists()) {
                    tempPartFile.delete()
                }

                var connection: HttpURLConnection? = null
                var inputStream: InputStream? = null
                var outputStream: FileOutputStream? = null

                try {
                    connection = (url.openConnection() as HttpURLConnection).apply {
                        connectTimeout = CONNECT_TIMEOUT_MS
                        readTimeout = READ_TIMEOUT_MS
                        instanceFollowRedirects = true
                        setRequestProperty("User-Agent", "Echolet-Android/0.1.0")
                    }

                    val responseCode = connection.responseCode
                    if (responseCode !in 200..299) {
                        throw IllegalStateException("HTTP error $responseCode: ${connection.responseMessage}")
                    }

                    val totalBytes = connection.contentLengthLong
                    inputStream = connection.inputStream
                    outputStream = FileOutputStream(tempPartFile)

                    val buffer = ByteArray(BUFFER_SIZE)
                    var bytesRead: Int
                    var totalDownloaded = 0L
                    var lastReportTime = System.currentTimeMillis()
                    var lastReportBytes = 0L

                    NativeBridge.nativeSetDownloadProgress(
                        modelId,
                        0L,
                        if (totalBytes > 0) totalBytes else 0L,
                        "downloading",
                    )
                    onProgressOrStatusChanged()

                    while (inputStream.read(buffer).also { bytesRead = it } != -1) {
                        outputStream.write(buffer, 0, bytesRead)
                        totalDownloaded += bytesRead

                        val now = System.currentTimeMillis()
                        // Report throttle: at most once per 200ms or 512KB
                        if (now - lastReportTime >= PROGRESS_INTERVAL_MS || totalDownloaded - lastReportBytes >= 512 * 1024) {
                            lastReportTime = now
                            lastReportBytes = totalDownloaded
                            NativeBridge.nativeSetDownloadProgress(
                                modelId,
                                totalDownloaded,
                                if (totalBytes > 0) totalBytes else 0L,
                                "downloading",
                            )
                            onProgressOrStatusChanged()
                        }
                    }

                    outputStream.flush()

                    NativeBridge.nativeSetDownloadProgress(
                        modelId,
                        totalDownloaded,
                        if (totalBytes > 0) totalBytes else totalDownloaded,
                        "verifying",
                    )
                    onProgressOrStatusChanged()

                } finally {
                    try { outputStream?.close() } catch (_: Throwable) {}
                    try { inputStream?.close() } catch (_: Throwable) {}
                    try { connection?.disconnect() } catch (_: Throwable) {}
                }

                // Delegate verification & atomic install to Rust
                NativeBridge.nativeSetDownloadProgress(modelId, 0, 0, "installing")
                onProgressOrStatusChanged()

                val installOk = NativeBridge.nativeInstallModelFromArchive(
                    modelId,
                    tempPartFile.absolutePath,
                )

                if (!installOk) {
                    throw IllegalStateException("Rust native install failed for $modelId")
                }

                NativeBridge.nativeSetDownloadProgress(modelId, 0, 0, "completed")
                Log.i(TAG, "Model $modelId successfully installed!")

            } catch (t: Throwable) {
                Log.e(TAG, "Failed to download/install model $modelId", t)
                NativeBridge.nativeSetDownloadProgress(modelId, 0, 0, "failed")
            } finally {
                activeDownloads.remove(modelId)
                try {
                    tempPartFile?.let { if (it.exists()) it.delete() }
                } catch (t: Throwable) {
                    Log.w(TAG, "Failed to clean up temp file $tempPartFile", t)
                }
                onProgressOrStatusChanged()
            }
        }
    }

    companion object {
        private const val TAG = "ModelDownloadManager"
        private const val CONNECT_TIMEOUT_MS = 30_000
        private const val READ_TIMEOUT_MS = 30_000
        private const val BUFFER_SIZE = 64 * 1024
        private const val PROGRESS_INTERVAL_MS = 250L
    }
}
