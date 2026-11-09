package com.voiceme.recorder

import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.util.zip.Deflater
import java.util.zip.ZipEntry
import java.util.zip.ZipInputStream
import java.util.zip.ZipOutputStream

/** Minimal JSON for the two tiny documents the zips carry. */
internal object Json {
    fun escape(s: String): String = buildString {
        for (c in s) {
            when {
                c == '"' -> append("\\\"")
                c == '\\' -> append("\\\\")
                c == '\n' -> append("\\n")
                c == '\r' -> append("\\r")
                c == '\t' -> append("\\t")
                c < ' ' -> append("\\u%04x".format(c.code))
                else -> append(c)
            }
        }
    }

    private val ESCAPED = Regex("\\\\(.)")

    /** The string value of `"key"` in a flat object, or null. */
    fun string(json: String, key: String): String? {
        val pattern = "\"" + Regex.escape(key) + "\"\\s*:\\s*\"((?:[^\"\\\\]|\\\\.)*)\""
        val match = Regex(pattern).find(json) ?: return null
        return match.groupValues[1].replace(ESCAPED) { r ->
            when (val c = r.groupValues[1][0]) {
                'n' -> "\n"
                'r' -> "\r"
                't' -> "\t"
                else -> c.toString()
            }
        }
    }
}

/**
 * Recordings kept in a folder: `wavs/<id>.wav` per sentence, written the
 * moment a take stops. Zips use the desktop recorder's layout exactly:
 * `metadata.csv`, `wavs/`, `manifest.json` (+ `resume.json` for a backup).
 */
class Store(private val dir: File) {
    private val wavs = File(dir, "wavs").apply { mkdirs() }

    fun wavFile(id: String) = File(wavs, "$id.wav")

    /** id -> seconds, for every recording on disk. */
    fun recorded(): MutableMap<String, Float> {
        val map = HashMap<String, Float>()
        wavs.listFiles { f -> f.extension == "wav" }?.forEach { f ->
            map[f.nameWithoutExtension] = (f.length() - 44).coerceAtLeast(0) / 2f / Dsp.TARGET_RATE
        }
        return map
    }

    /** Written aside, then renamed over, so a half-written file never counts. */
    fun save(id: String, wav: ByteArray) {
        val part = File(wavs, "$id.wav.part")
        part.writeBytes(wav)
        wavFile(id).delete()
        check(part.renameTo(wavFile(id))) { "could not save $id" }
    }

    fun delete(id: String) {
        wavFile(id).delete()
    }

    private fun text(name: String): String? =
        File(dir, name).takeIf { it.isFile }?.readText()?.trim()?.takeIf { it.isNotEmpty() }

    var speaker: String
        get() = text("speaker.txt") ?: "speaker"
        set(value) = File(dir, "speaker.txt").writeText(value.trim())

    val hasSpeaker get() = File(dir, "speaker.txt").isFile

    var mic: String?
        get() = text("mic.txt")
        set(value) {
            val f = File(dir, "mic.txt")
            if (value == null) f.delete() else f.writeText(value)
        }

    class Summary(val count: Int, val seconds: Double, val skipped: Int, val fileName: String)

    private fun slug(name: String): String {
        val s = name.map { if (it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9') it.lowercaseChar() else '-' }
            .joinToString("").trim('-')
        return s.ifEmpty { "speaker" }
    }

    /** The zip's name for `count` recordings; `backup` is the resumable one. */
    fun zipName(count: Int, backup: Boolean) =
        "voice-me-${if (backup) "backup" else "dataset"}-${slug(speaker)}-$count.zip"

    /** How many sentences in [sentences] have a recording (what an export would hold). */
    fun exportable(sentences: List<Sentence>) = sentences.count { wavFile(it.id).isFile }

    /**
     * Write the zip of every sentence in [sentences] that has a recording.
     * [nextId] makes it a backup (adds `resume.json`).
     */
    fun export(sentences: List<Sentence>, out: OutputStream, nextId: String?): Summary {
        val metadata = StringBuilder()
        val ids = ArrayList<String>()
        var seconds = 0.0
        var skipped = 0
        for (s in sentences) {
            val f = wavFile(s.id)
            if (!f.isFile) continue
            val length = Dsp.wavSeconds(f.readBytes())
            if (length == null) {
                skipped++
                continue
            }
            seconds += length
            metadata.append("${s.id}.wav|${s.text}\n")
            ids.add(s.id)
        }
        require(ids.isNotEmpty()) { "Henüz dışa aktarılacak kayıt yok." }
        ZipOutputStream(out.buffered()).use { zip ->
            zip.setLevel(Deflater.DEFAULT_COMPRESSION)
            fun put(name: String, bytes: ByteArray) {
                zip.putNextEntry(ZipEntry(name))
                zip.write(bytes)
                zip.closeEntry()
            }
            put(
                "manifest.json",
                ("{\"format\":1,\"sample_rate\":${Dsp.TARGET_RATE},\"language\":\"tr\",\"phoneme_type\":\"dizge\"," +
                    "\"speaker\":\"${Json.escape(speaker)}\",\"count\":${ids.size}," +
                    "\"seconds\":${Math.round(seconds * 10) / 10.0}}").toByteArray(),
            )
            put("metadata.csv", metadata.toString().toByteArray())
            if (nextId != null) {
                put("resume.json", "{\"format\":1,\"next_id\":\"${Json.escape(nextId)}\"}".toByteArray())
            }
            for (id in ids) put("wavs/$id.wav", wavFile(id).readBytes())
        }
        return Summary(ids.size, seconds, skipped, zipName(ids.size, nextId != null))
    }

    class Imported(val added: Int, val kept: Int, val skipped: Int, val nextId: String?)

    private fun safeId(id: String) =
        id.isNotEmpty() && id.all { it.isLetterOrDigit() || it == '_' || it == '-' }

    /** Merge a training or backup zip; an existing take is never overwritten. */
    fun import(input: InputStream): Imported {
        val have = recorded().keys
        var added = 0
        var kept = 0
        var skipped = 0
        var nextId: String? = null
        ZipInputStream(input.buffered()).use { zip ->
            while (true) {
                val entry = zip.nextEntry ?: break
                val name = entry.name
                when {
                    name == "manifest.json" -> {
                        val found = Json.string(String(zip.readBytes()), "speaker").orEmpty()
                        if (!hasSpeaker && found.isNotBlank()) speaker = found
                    }
                    name == "resume.json" -> {
                        nextId = Json.string(String(zip.readBytes()), "next_id")?.takeIf { safeId(it) }
                    }
                    name.startsWith("wavs/") && name.endsWith(".wav") -> {
                        val id = name.removePrefix("wavs/").removeSuffix(".wav")
                        if (!safeId(id)) {
                            skipped++
                        } else if (id in have) {
                            kept++
                        } else {
                            val bytes = zip.readBytes()
                            if (Dsp.wavSeconds(bytes) == null) {
                                skipped++
                            } else {
                                save(id, bytes)
                                added++
                            }
                        }
                    }
                }
            }
        }
        return Imported(added, kept, skipped, nextId)
    }
}
