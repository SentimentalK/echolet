package com.mainstayx.echolet

import org.json.JSONArray
import org.json.JSONObject

/**
 * Parsed data representation of a single model row in the Android model browser.
 */
data class ModelItemUi(
    val id: String,
    val label: String,
    val releaseDate: String,
    val verificationLabel: String,
    val isVerified: Boolean,
    val selected: Boolean,
    val installed: Boolean,
    val downloadPhase: String,
    val downloadLabel: String?,
    val progressPercent: Int?,
    val primaryAction: String, // "None", "Download", "RetryDownload", "Select"
    val enabled: Boolean,
)

/**
 * Parsed data representation of a language/capability group of models.
 */
data class ModelGroupUi(
    val id: String,
    val label: String,
    val models: List<ModelItemUi>,
)

/**
 * Parsed model snapshot for the Android UI.
 */
data class ModelSnapshotUi(
    val schemaVersion: Int,
    val selectedModelId: String,
    val selectedModelDir: String?,
    val runtimeState: String,
    val groups: List<ModelGroupUi>,
) {
    val selectedModelLabel: String?
        get() = groups.flatMap { it.models }.firstOrNull { it.id == selectedModelId }?.label

    val hasSelectedAndInstalledModel: Boolean
        get() = selectedModelDir != null && groups.flatMap { it.models }.any { it.id == selectedModelId && it.installed }

    companion object {
        fun parseJson(jsonStr: String): ModelSnapshotUi {
            val root = JSONObject(jsonStr)
            val schemaVersion = root.optInt("schema_version", 2)
            val selectedModelId = root.optString("selected_model_id", "")
            val selectedModelDir = if (root.has("selected_model_dir") && !root.isNull("selected_model_dir")) {
                root.getString("selected_model_dir")
            } else null
            val runtimeState = root.optString("runtime_state", "NoModel")

            val groupsArray = root.optJSONArray("model_groups") ?: JSONArray()
            val groups = mutableListOf<ModelGroupUi>()

            for (i in 0 until groupsArray.length()) {
                val groupObj = groupsArray.getJSONObject(i)
                val gId = groupObj.optString("id", "")
                val gLabel = groupObj.optString("label", "")
                val modelsArray = groupObj.optJSONArray("models") ?: JSONArray()
                val models = mutableListOf<ModelItemUi>()

                for (j in 0 until modelsArray.length()) {
                    val mObj = modelsArray.getJSONObject(j)
                    val mId = mObj.optString("id", "")
                    val mLabel = mObj.optString("label", "")
                    val releaseDate = mObj.optString("release_date", "")
                    val verificationLabel = mObj.optString("verification_label", "")
                    val isVerified = mObj.optBoolean("is_verified", false)
                    val selected = mObj.optBoolean("selected", false)
                    val installed = mObj.optBoolean("installed", false)

                    val dlObj = mObj.optJSONObject("download")
                    val dlPhase = dlObj?.optString("phase", "NotDownloading") ?: "NotDownloading"
                    val dlLabel = if (dlObj != null && dlObj.has("label") && !dlObj.isNull("label")) {
                        dlObj.getString("label")
                    } else null
                    val progressPercent = if (dlObj != null && dlObj.has("progress_percent") && !dlObj.isNull("progress_percent")) {
                        dlObj.getInt("progress_percent")
                    } else null

                    val primaryAction = mObj.optString("primary_action", "None")
                    val enabled = mObj.optBoolean("enabled", true)

                    models.add(
                        ModelItemUi(
                            id = mId,
                            label = mLabel,
                            releaseDate = releaseDate,
                            verificationLabel = verificationLabel,
                            isVerified = isVerified,
                            selected = selected,
                            installed = installed,
                            downloadPhase = dlPhase,
                            downloadLabel = dlLabel,
                            progressPercent = progressPercent,
                            primaryAction = primaryAction,
                            enabled = enabled,
                        )
                    )
                }

                groups.add(ModelGroupUi(id = gId, label = gLabel, models = models))
            }

            return ModelSnapshotUi(
                schemaVersion = schemaVersion,
                selectedModelId = selectedModelId,
                selectedModelDir = selectedModelDir,
                runtimeState = runtimeState,
                groups = groups,
            )
        }
    }
}
