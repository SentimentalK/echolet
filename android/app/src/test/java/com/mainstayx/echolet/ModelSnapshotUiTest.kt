package com.mainstayx.echolet

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for Phase 1-B Model Snapshot parsing and Model Presenter state.
 */
class ModelSnapshotUiTest {

    private val sampleJson = """
    {
      "schema_version": 2,
      "selected_model_id": "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
      "selected_model_dir": "/data/user/0/com.mainstayx.echolet/files/models/bilingual-zh-en",
      "runtime_state": "Ready",
      "model_groups": [
        {
          "id": "bilingual",
          "label": "Chinese + English",
          "models": [
            {
              "id": "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
              "label": "X-ASR 0.16B",
              "release_date": "2026-05-18",
              "verification_label": "Echolet Verified",
              "is_verified": true,
              "selected": true,
              "installed": true,
              "download": {
                "phase": "Completed",
                "label": "Installed",
                "progress_fraction": "1.00",
                "progress_percent": 100,
                "retryable": false
              },
              "primary_action": "None",
              "enabled": false
            }
          ]
        },
        {
          "id": "english",
          "label": "English",
          "models": [
            {
              "id": "echolet-kroko-streaming-en-2025-08-06-r1",
              "label": "Kroko English Streaming 0.066B",
              "release_date": "2025-08-06",
              "verification_label": "Experimental",
              "is_verified": false,
              "selected": false,
              "installed": false,
              "download": {
                "phase": "Downloading",
                "label": "Downloading 45%",
                "progress_fraction": "0.45",
                "progress_percent": 45,
                "retryable": false
              },
              "primary_action": "None",
              "enabled": false
            },
            {
              "id": "echolet-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25-r1",
              "label": "Nemotron Speech Streaming 0.6B",
              "release_date": "2026-04-25",
              "verification_label": "Experimental",
              "is_verified": false,
              "selected": false,
              "installed": false,
              "download": {
                "phase": "NotDownloading",
                "label": null,
                "progress_fraction": null,
                "progress_percent": null,
                "retryable": false
              },
              "primary_action": "Download",
              "enabled": true
            },
            {
              "id": "echolet-parakeet-unified-en-0.6b-560ms-int8-2026-05-12-r1",
              "label": "Parakeet Unified 0.6B",
              "release_date": "2026-05-12",
              "verification_label": "Experimental",
              "is_verified": false,
              "selected": false,
              "installed": true,
              "download": {
                "phase": "Completed",
                "label": "Installed",
                "progress_fraction": "1.00",
                "progress_percent": 100,
                "retryable": false
              },
              "primary_action": "Select",
              "enabled": true
            }
          ]
        }
      ]
    }
    """.trimIndent()

    @Test
    fun `parses snapshot JSON into structured UI groups and models`() {
        val snapshot = ModelSnapshotUi.parseJson(sampleJson)
        assertEquals(2, snapshot.schemaVersion)
        assertEquals("echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1", snapshot.selectedModelId)
        assertEquals("/data/user/0/com.mainstayx.echolet/files/models/bilingual-zh-en", snapshot.selectedModelDir)
        assertEquals("Ready", snapshot.runtimeState)
        assertEquals(2, snapshot.groups.size)

        val zhEnGroup = snapshot.groups[0]
        assertEquals("bilingual", zhEnGroup.id)
        assertEquals("Chinese + English", zhEnGroup.label)
        assertEquals(1, zhEnGroup.models.size)

        val xasr = zhEnGroup.models[0]
        assertEquals("X-ASR 0.16B", xasr.label)
        assertTrue(xasr.isVerified)
        assertTrue(xasr.selected)
        assertTrue(xasr.installed)
        assertEquals("Completed", xasr.downloadPhase)
        assertEquals(100, xasr.progressPercent)

        val enGroup = snapshot.groups[1]
        assertEquals("english", enGroup.id)
        assertEquals(3, enGroup.models.size)

        val kroko = enGroup.models[0]
        assertEquals("Kroko English Streaming 0.066B", kroko.label)
        assertFalse(kroko.isVerified)
        assertFalse(kroko.selected)
        assertFalse(kroko.installed)
        assertEquals("Downloading", kroko.downloadPhase)
        assertEquals("Downloading 45%", kroko.downloadLabel)
        assertEquals(45, kroko.progressPercent)

        val nemo = enGroup.models[1]
        assertEquals("Download", nemo.primaryAction)
        assertTrue(nemo.enabled)

        val parakeet = enGroup.models[2]
        assertEquals("Select", parakeet.primaryAction)
        assertTrue(parakeet.enabled)
        assertTrue(parakeet.installed)

        assertEquals("X-ASR 0.16B", snapshot.selectedModelLabel)
        assertTrue(snapshot.hasSelectedAndInstalledModel)
    }

    @Test
    fun `presenter updates plan with model snapshot in Expanded mode`() {
        val presenter = ImeKeyboardPresenter(onSetupRequested = {})
        val ready = ImePrerequisites(permissionGranted = true, modelStaged = true, nativeReady = true)
        presenter.onFreshVisibility(ready)

        // Switch to Expanded
        val expandedPlan = presenter.onToggleExpand()
        assertEquals(ImeLayoutMode.EXPANDED, expandedPlan.layout)
        assertNotNull(expandedPlan.expandedPanel)
        assertNull(expandedPlan.expandedPanel!!.modelSnapshot)

        // Provide snapshot
        val snapshot = ModelSnapshotUi.parseJson(sampleJson)
        val updatedPlan = presenter.updateModelSnapshot(snapshot)
        assertEquals(ImeLayoutMode.EXPANDED, updatedPlan.layout)
        assertNotNull(updatedPlan.expandedPanel!!.modelSnapshot)
        assertEquals(2, updatedPlan.expandedPanel!!.modelSnapshot!!.groups.size)
    }

    @Test
    fun `parses snapshot with selected but uninstalled default model`() {
        val missingSelectedJson = """
        {
          "schema_version": 2,
          "selected_model_id": "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
          "selected_model_dir": null,
          "runtime_state": "NoModel",
          "model_groups": [
            {
              "id": "bilingual",
              "label": "Chinese + English",
              "models": [
                {
                  "id": "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
                  "label": "X-ASR 0.16B",
                  "release_date": "2026-05-18",
                  "verification_label": "Echolet Verified",
                  "is_verified": true,
                  "selected": true,
                  "installed": false,
                  "download": {
                    "phase": "NotDownloading",
                    "label": null,
                    "progress_fraction": null,
                    "progress_percent": null,
                    "retryable": false
                  },
                  "primary_action": "Download",
                  "enabled": true
                }
              ]
            }
          ]
        }
        """.trimIndent()

        val snapshot = ModelSnapshotUi.parseJson(missingSelectedJson)
        assertEquals("echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1", snapshot.selectedModelId)
        assertNull(snapshot.selectedModelDir)
        assertFalse(snapshot.hasSelectedAndInstalledModel)
        assertEquals(1, snapshot.groups.size)

        val xasr = snapshot.groups[0].models[0]
        assertTrue(xasr.selected)
        assertFalse(xasr.installed)
        assertEquals("Download", xasr.primaryAction)
        assertTrue(xasr.enabled)
    }

    @Test
    fun `listening state snapshot disables action for selected uninstalled model`() {
        val listeningMissingJson = """
        {
          "schema_version": 2,
          "selected_model_id": "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
          "selected_model_dir": null,
          "runtime_state": "Listening",
          "model_groups": [
            {
              "id": "bilingual",
              "label": "Chinese + English",
              "models": [
                {
                  "id": "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
                  "label": "X-ASR 0.16B",
                  "release_date": "2026-05-18",
                  "verification_label": "Echolet Verified",
                  "is_verified": true,
                  "selected": true,
                  "installed": false,
                  "download": {
                    "phase": "NotDownloading",
                    "label": null,
                    "progress_fraction": null,
                    "progress_percent": null,
                    "retryable": false
                  },
                  "primary_action": "Download",
                  "enabled": false
                }
              ]
            }
          ]
        }
        """.trimIndent()

        val snapshot = ModelSnapshotUi.parseJson(listeningMissingJson)
        assertEquals("Listening", snapshot.runtimeState)
        val xasr = snapshot.groups[0].models[0]
        assertTrue(xasr.selected)
        assertFalse(xasr.installed)
        assertEquals("Download", xasr.primaryAction)
        assertFalse(xasr.enabled)
    }
}
