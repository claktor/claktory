package com.claktory.app

import org.json.JSONException
import org.json.JSONObject
import org.json.JSONTokener
import java.io.File
import java.io.IOException

/**
 * Unsent drafts by agent (app/lib/drafts.dart), one of the three things
 * kept on the device by the person's decision. A private file of their
 * own, written atomically and out of every backup like the credentials,
 * but not sealed with their key and never cleared with them (DESIGN.md
 * §5): a draft that can't be read is only a draft lost, never a reason to
 * fail.
 */
class DraftStore(private val file: File, private val log: (String) -> Unit = {}) {
    @Synchronized
    fun read(): Map<String, String> {
        if (!file.exists()) return emptyMap()
        return try {
            val json = JSONTokener(file.readText(Charsets.UTF_8)).nextValue() as? JSONObject
                ?: throw JSONException("not an object")
            buildMap {
                for (key in json.keys()) json.opt(key).let { if (it is String) put(key, it) }
            }
        } catch (error: Exception) {
            if (error !is IOException && error !is JSONException) throw error
            log("couldn't read the drafts (${error.javaClass.simpleName})")
            emptyMap()
        }
    }

    @Synchronized
    fun write(drafts: Map<*, *>) {
        val json = JSONObject()
        for ((key, value) in drafts) if (key is String && value is String) json.put(key, value)
        try {
            writeAtomically(file, json.toString().toByteArray(Charsets.UTF_8))
        } catch (error: IOException) {
            log("couldn't keep the drafts (${error.javaClass.simpleName})")
        }
    }

    companion object {
        const val FILE_NAME = "drafts.json"
    }
}
