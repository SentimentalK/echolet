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
 * Seam for native model operations, allowing JVM unit testing without loading .so libraries.
 */
interface NativeModelAdapter {
    fun getDownloadSpec(modelId: String): String?
    fun setDownloadProgress(modelId: String, downloadedBytes: Long, totalBytes: Long, phase: String)
    fun installModelFromArchive(modelId: String, archivePath: String): Boolean
}

object DefaultNativeModelAdapter : NativeModelAdapter {
    override fun getDownloadSpec(modelId: String): String? =
        NativeBridge.nativeGetModelDownloadSpec(modelId)

    override fun setDownloadProgress(
        modelId: String,
        downloadedBytes: Long,
        totalBytes: Long,
        phase: String,
    ) = NativeBridge.nativeSetDownloadProgress(modelId, downloadedBytes, totalBytes, phase)

    override fun installModelFromArchive(modelId: String, archivePath: String): Boolean =
        NativeBridge.nativeInstallModelFromArchive(modelId, archivePath)
}

/**
 * Canonical PascalCase download phases matching Rust [`echolet::models::progress::DownloadStatus`].
 */
enum class DownloadPhase(val wireName: String) {
    STARTING("Starting"),
    DOWNLOADING("Downloading"),
    VERIFYING("Verifying"),
    EXTRACTING("Extracting"),
    INSTALLING("Installing"),
    COMPLETED("Completed"),
    FAILED("Failed");

    companion object {
        fun fromWireName(name: String): DownloadPhase? =
            entries.firstOrNull { it.wireName == name }
    }
}

/**
 * Typed download specification sourced solely from the canonical Rust ModelRegistry.
 */
data class ModelDownloadSpec(
    val modelId: String,
    val url: String,
    val sha256: String,
    val downloadSizeBytes: Long?,
    val installedSizeBytes: Long?,
)

/**
 * Android Model Download and Installation Manager (Phase 1-B).
 *
 * Responsibilities:
 * - Obtains canonical model download URLs & metadata solely from Rust JNI [NativeBridge.nativeGetModelDownloadSpec].
 * - Downloads catalog models via standard HTTPS [HttpURLConnection] into temporary `.part` files.
 * - Follows HTTPS redirects safely (rejects non-HTTPS redirect targets).
 * - Periodically updates JNI progress via [NativeBridge.nativeSetDownloadProgress] using exact PascalCase phases.
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
    private val nativeAdapter: NativeModelAdapter = DefaultNativeModelAdapter,
    onProgressOrStatusChanged: () -> Unit = {},
) {

    private val observer = java.util.concurrent.atomic.AtomicReference<(() -> Unit)?>(onProgressOrStatusChanged)

    fun setObserver(callback: (() -> Unit)?) {
        observer.set(callback)
    }

    fun detachObserver() {
        observer.set(null)
    }

    fun clearObserver() {
        detachObserver()
    }

    fun hasObserver(): Boolean = observer.get() != null

    private fun notifyObserver() {
        try {
            observer.get()?.invoke()
        } catch (t: Throwable) {
            Log.w(TAG, "Observer callback threw exception", t)
        }
    }

    private val activeDownloads = ConcurrentHashMap<String, Boolean>()

    fun isDownloading(modelId: String): Boolean = activeDownloads[modelId] == true

    /**
     * Resolves typed download specification for [modelId] from the authoritative
     * canonical registry via JNI. Never uses hardcoded Kotlin when-switches.
     */
    fun getDownloadSpec(modelId: String): ModelDownloadSpec? {
        val json = try {
            nativeAdapter.getDownloadSpec(modelId)
        } catch (t: Throwable) {
            Log.e(TAG, "Failed to get model download spec for $modelId", t)
            null
        } ?: return null

        return try {
            val obj = JSONObject(json)
            ModelDownloadSpec(
                modelId = obj.getString("model_id"),
                url = obj.getString("url"),
                sha256 = obj.getString("sha256"),
                downloadSizeBytes = if (obj.has("download_size_bytes") && !obj.isNull("download_size_bytes")) {
                    obj.getLong("download_size_bytes")
                } else null,
                installedSizeBytes = if (obj.has("installed_size_bytes") && !obj.isNull("installed_size_bytes")) {
                    obj.getLong("installed_size_bytes")
                } else null,
            )
        } catch (e: Exception) {
            Log.e(TAG, "Failed to parse download spec JSON for $modelId: $json", e)
            null
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

        val spec = getDownloadSpec(modelId)
        if (spec == null) {
            Log.e(TAG, "Unknown or non-downloadable model ID: $modelId")
            activeDownloads.remove(modelId)
            try {
                nativeAdapter.setDownloadProgress(modelId, 0, 0, DownloadPhase.FAILED.wireName)
            } catch (t: Throwable) {
                Log.w(TAG, "Failed to report failed progress", t)
            }
            notifyObserver()
            return
        }

        executor.execute {
            var tempPartFile: File? = null
            try {
                nativeAdapter.setDownloadProgress(modelId, 0, 0, DownloadPhase.STARTING.wireName)
                notifyObserver()

                val initialUrl = spec.url
                if (!initialUrl.startsWith("https://", ignoreCase = true)) {
                    throw IllegalArgumentException("Only HTTPS downloads are supported: $initialUrl")
                }

                val cacheDir = try {
                    context.cacheDir ?: context.filesDir
                } catch (_: Throwable) {
                    null
                } ?: File(System.getProperty("java.io.tmpdir"), "echolet-test-cache").apply { mkdirs() }
                val ext = if (initialUrl.endsWith(".tar.zst")) ".tar.zst" else ".tar.bz2"
                tempPartFile = File(cacheDir, "echolet-dl-$modelId-$ext.part")
                if (tempPartFile.exists()) {
                    tempPartFile.delete()
                }

                var connection: HttpURLConnection? = null
                var inputStream: InputStream? = null
                var outputStream: FileOutputStream? = null

                try {
                    // Safe HTTPS redirect loop (capped at MAX_REDIRECTS)
                    var currentUrl = initialUrl
                    var hops = 0
                    while (true) {
                        val parsedUrl = URL(currentUrl)
                        if (!parsedUrl.protocol.equals("https", ignoreCase = true)) {
                            throw IllegalArgumentException("Refusing non-HTTPS redirect to: $currentUrl")
                        }

                        val conn = (parsedUrl.openConnection() as HttpURLConnection).apply {
                            connectTimeout = CONNECT_TIMEOUT_MS
                            readTimeout = READ_TIMEOUT_MS
                            instanceFollowRedirects = false
                            setRequestProperty("User-Agent", "Echolet-Android/0.1.0")
                        }

                        val responseCode = conn.responseCode
                        if (responseCode in 300..399) {
                            conn.disconnect()
                            hops++
                            if (hops > MAX_REDIRECTS) {
                                throw IllegalStateException("Too many HTTP redirects (exceeded $MAX_REDIRECTS)")
                            }
                            val location = conn.getHeaderField("Location")
                                ?: throw IllegalStateException("HTTP redirect $responseCode missing Location header")
                            currentUrl = URL(parsedUrl, location).toString()
                            continue
                        }

                        if (responseCode !in 200..299) {
                            throw IllegalStateException("HTTP error $responseCode: ${conn.responseMessage}")
                        }

                        connection = conn
                        break
                    }

                    val totalBytes = connection.contentLengthLong
                    // Bounded content length check (reject downloads > 2GB)
                    if (totalBytes > MAX_DOWNLOAD_BYTES) {
                        throw IllegalStateException("Advertised download size ($totalBytes bytes) exceeds maximum limit")
                    }

                    inputStream = connection.inputStream
                    outputStream = FileOutputStream(tempPartFile)

                    val buffer = ByteArray(BUFFER_SIZE)
                    var bytesRead: Int
                    var totalDownloaded = 0L
                    var lastReportTime = System.currentTimeMillis()
                    var lastReportBytes = 0L

                    nativeAdapter.setDownloadProgress(
                        modelId,
                        0L,
                        if (totalBytes > 0) totalBytes else 0L,
                        DownloadPhase.DOWNLOADING.wireName,
                    )
                    notifyObserver()

                    while (inputStream.read(buffer).also { bytesRead = it } != -1) {
                        outputStream.write(buffer, 0, bytesRead)
                        totalDownloaded += bytesRead

                        if (totalDownloaded > MAX_DOWNLOAD_BYTES) {
                            throw IllegalStateException("Downloaded bytes ($totalDownloaded) exceeded maximum limit")
                        }

                        val now = System.currentTimeMillis()
                        // Report throttle: at most once per 250ms or 512KB
                        if (now - lastReportTime >= PROGRESS_INTERVAL_MS || totalDownloaded - lastReportBytes >= 512 * 1024) {
                            lastReportTime = now
                            lastReportBytes = totalDownloaded
                            nativeAdapter.setDownloadProgress(
                                modelId,
                                totalDownloaded,
                                if (totalBytes > 0) totalBytes else 0L,
                                DownloadPhase.DOWNLOADING.wireName,
                            )
                            notifyObserver()
                        }
                    }

                    outputStream.flush()

                    nativeAdapter.setDownloadProgress(
                        modelId,
                        totalDownloaded,
                        if (totalBytes > 0) totalBytes else totalDownloaded,
                        DownloadPhase.VERIFYING.wireName,
                    )
                    notifyObserver()

                } finally {
                    try { outputStream?.close() } catch (_: Throwable) {}
                    try { inputStream?.close() } catch (_: Throwable) {}
                    try { connection?.disconnect() } catch (_: Throwable) {}
                }

                // Delegate verification & atomic install to Rust
                nativeAdapter.setDownloadProgress(modelId, 0, 0, DownloadPhase.INSTALLING.wireName)
                notifyObserver()

                val installOk = nativeAdapter.installModelFromArchive(
                    modelId,
                    tempPartFile.absolutePath,
                )

                if (!installOk) {
                    throw IllegalStateException("Rust native install failed for $modelId")
                }

                nativeAdapter.setDownloadProgress(modelId, 0, 0, DownloadPhase.COMPLETED.wireName)
                Log.i(TAG, "Model $modelId successfully installed!")

            } catch (t: Throwable) {
                Log.e(TAG, "Failed to download/install model $modelId", t)
                try {
                    nativeAdapter.setDownloadProgress(modelId, 0, 0, DownloadPhase.FAILED.wireName)
                } catch (reportErr: Throwable) {
                    Log.w(TAG, "Failed to report failure status", reportErr)
                }
            } finally {
                activeDownloads.remove(modelId)
                try {
                    tempPartFile?.let { if (it.exists()) it.delete() }
                } catch (t: Throwable) {
                    Log.w(TAG, "Failed to clean up temp file $tempPartFile", t)
                }
                notifyObserver()
            }
        }
    }

    companion object {
        private const val TAG = "ModelDownloadManager"
        private const val CONNECT_TIMEOUT_MS = 30_000
        private const val READ_TIMEOUT_MS = 30_000
        private const val BUFFER_SIZE = 64 * 1024
        private const val PROGRESS_INTERVAL_MS = 250L
        private const val MAX_REDIRECTS = 5
        private const val MAX_DOWNLOAD_BYTES = 2L * 1024 * 1024 * 1024 // 2 GB
    }
}
