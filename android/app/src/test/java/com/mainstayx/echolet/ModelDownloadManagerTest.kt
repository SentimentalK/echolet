package com.mainstayx.echolet

import android.content.Context
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.lang.reflect.Proxy
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/**
 * JVM unit tests for [ModelDownloadManager] contract, PascalCase wire progress reporting,
 * and canonical registry download spec integration.
 */
class ModelDownloadManagerTest {

    private class FakeContext(private val baseDir: File) : android.content.ContextWrapper(null) {
        override fun getCacheDir(): File = baseDir
        override fun getFilesDir(): File = baseDir
    }

    private fun dummyContext(): Context {
        val dir = File(System.getProperty("java.io.tmpdir"), "echolet-test-ctx").apply { mkdirs() }
        return FakeContext(dir)
    }

    private class RecordingNativeModelAdapter : NativeModelAdapter {
        val specs = mutableMapOf<String, String>()
        val progressReports = ConcurrentLinkedQueue<ProgressReport>()
        var installOutcome = true
        val installInvocations = AtomicInteger(0)

        data class ProgressReport(
            val modelId: String,
            val downloadedBytes: Long,
            val totalBytes: Long,
            val phase: String,
        )

        override fun getDownloadSpec(modelId: String): String? = specs[modelId]

        override fun setDownloadProgress(
            modelId: String,
            downloadedBytes: Long,
            totalBytes: Long,
            phase: String,
        ) {
            progressReports.add(ProgressReport(modelId, downloadedBytes, totalBytes, phase))
        }

        override fun installModelFromArchive(modelId: String, archivePath: String): Boolean {
            installInvocations.incrementAndGet()
            return installOutcome
        }
    }

    private class DirectExecutor : Executor {
        override fun execute(command: Runnable) {
            command.run()
        }
    }

    @Test
    fun download_phase_wire_names_match_rust_pascal_case_exactly() {
        val expected = mapOf(
            DownloadPhase.STARTING to "Starting",
            DownloadPhase.DOWNLOADING to "Downloading",
            DownloadPhase.VERIFYING to "Verifying",
            DownloadPhase.EXTRACTING to "Extracting",
            DownloadPhase.INSTALLING to "Installing",
            DownloadPhase.COMPLETED to "Completed",
            DownloadPhase.FAILED to "Failed",
        )

        for ((phase, name) in expected) {
            assertEquals(name, phase.wireName)
            assertEquals(phase, DownloadPhase.fromWireName(name))
        }

        // Lowercase or arbitrary names must fail to resolve
        assertNull(DownloadPhase.fromWireName("starting"))
        assertNull(DownloadPhase.fromWireName("downloading"))
        assertNull(DownloadPhase.fromWireName("verifying"))
        assertNull(DownloadPhase.fromWireName("installing"))
        assertNull(DownloadPhase.fromWireName("completed"))
        assertNull(DownloadPhase.fromWireName("failed"))
        assertNull(DownloadPhase.fromWireName("invalid"))
    }

    @Test
    fun get_download_spec_parses_json_without_hardcoded_model_id_mapping() {
        val adapter = RecordingNativeModelAdapter()
        val customModelId = "custom-future-catalog-model-2027"
        adapter.specs[customModelId] = """
            {
              "model_id": "$customModelId",
              "url": "https://example.com/models/custom.tar.zst",
              "sha256": "abcd1234ef567890",
              "download_size_bytes": 1234567,
              "installed_size_bytes": 7654321
            }
        """.trimIndent()

        val manager = ModelDownloadManager(
            context = dummyContext(),
            executor = DirectExecutor(),
            nativeAdapter = adapter,
        )

        val spec = manager.getDownloadSpec(customModelId)
        assertNotNull(spec)
        assertEquals(customModelId, spec!!.modelId)
        assertEquals("https://example.com/models/custom.tar.zst", spec.url)
        assertEquals("abcd1234ef567890", spec.sha256)
        assertEquals(1234567L, spec.downloadSizeBytes)
        assertEquals(7654321L, spec.installedSizeBytes)
    }

    @Test
    fun start_download_for_unknown_model_reports_failed_and_does_not_leak_active_state() {
        val adapter = RecordingNativeModelAdapter()
        val manager = ModelDownloadManager(
            context = dummyContext(),
            executor = DirectExecutor(),
            nativeAdapter = adapter,
        )

        val unknownId = "unknown-model-id"
        manager.startDownload(unknownId)

        assertFalse(manager.isDownloading(unknownId))
        assertEquals(1, adapter.progressReports.size)
        val report = adapter.progressReports.single()
        assertEquals(unknownId, report.modelId)
        assertEquals(DownloadPhase.FAILED.wireName, report.phase)
    }

    @Test
    fun duplicate_start_download_call_is_rejected_while_in_flight() {
        val adapter = RecordingNativeModelAdapter()
        val modelId = "echolet-kroko-streaming-en-2025-08-06-r1"
        adapter.specs[modelId] = """
            {
              "model_id": "$modelId",
              "url": "https://example.com/kroko.tar.bz2",
              "sha256": "abcdef",
              "download_size_bytes": 1000,
              "installed_size_bytes": 2000
            }
        """.trimIndent()

        val gate = CountDownLatch(1)
        val startedGate = CountDownLatch(1)
        val blockingExecutor = Executor { command ->
            Thread {
                startedGate.countDown()
                gate.await()
                command.run()
            }.start()
        }

        val manager = ModelDownloadManager(
            context = dummyContext(),
            executor = blockingExecutor,
            nativeAdapter = adapter,
        )

        manager.startDownload(modelId)
        assertTrue(startedGate.await(5, TimeUnit.SECONDS))
        assertTrue(manager.isDownloading(modelId))

        // Second call while first is in-flight must be ignored
        manager.startDownload(modelId)
        assertTrue(manager.isDownloading(modelId))

        // Release the download thread
        gate.countDown()
    }

    @Test
    fun non_https_url_is_strictly_rejected() {
        val adapter = RecordingNativeModelAdapter()
        val modelId = "insecure-model"
        adapter.specs[modelId] = """
            {
              "model_id": "$modelId",
              "url": "http://insecure.example.com/model.tar.zst",
              "sha256": "abcdef",
              "download_size_bytes": 1000,
              "installed_size_bytes": 2000
            }
        """.trimIndent()

        val manager = ModelDownloadManager(
            context = dummyContext(),
            executor = DirectExecutor(),
            nativeAdapter = adapter,
        )

        manager.startDownload(modelId)
        assertFalse(manager.isDownloading(modelId))

        val finalReport = adapter.progressReports.last()
        assertEquals(DownloadPhase.FAILED.wireName, finalReport.phase)
    }
}
