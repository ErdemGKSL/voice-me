package com.voiceme.recorder

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.nio.file.Files
import java.util.zip.ZipInputStream
import kotlin.math.sin

class StoreTest {
    private fun store() = Store(Files.createTempDirectory("rec").toFile())
    private fun wav(seconds: Float) =
        Dsp.wavBytes(FloatArray((seconds * Dsp.TARGET_RATE).toInt()) { (0.5 * sin(it * 0.05)).toFloat() })

    private val sentences = listOf("1", "2", "3").map { Sentence(it, "metin $it") }

    @Test
    fun savedTakesAreFoundAgainAndDeleted() {
        val s = store()
        s.save("1", wav(1f))
        assertEquals(setOf("1"), s.recorded().keys)
        assertEquals(1f, s.recorded()["1"]!!, 0.01f)
        s.delete("1")
        assertTrue(s.recorded().isEmpty())
    }

    @Test
    fun exportHasTheDesktopLayout() {
        val s = store()
        s.speaker = "Erdem Göksel"
        s.save("2", wav(1f))
        s.save("1", wav(2f))
        val out = ByteArrayOutputStream()
        val summary = s.export(sentences, out, null)
        assertEquals(2, summary.count)
        assertEquals("voice-me-dataset-erdem-g-ksel-2.zip", summary.fileName)
        val entries = HashMap<String, String>()
        ZipInputStream(ByteArrayInputStream(out.toByteArray())).use { z ->
            while (true) {
                val e = z.nextEntry ?: break
                entries[e.name] = String(z.readBytes(), Charsets.ISO_8859_1)
            }
        }
        assertEquals(setOf("manifest.json", "metadata.csv", "wavs/1.wav", "wavs/2.wav"), entries.keys)
        assertEquals("1.wav|metin 1\n2.wav|metin 2\n", entries["metadata.csv"])
        assertTrue(entries["manifest.json"]!!.contains("\"phoneme_type\":\"dizge\""))
    }

    @Test
    fun aBackupCarriesTheWorkToAnotherStore() {
        val here = store()
        here.speaker = "erdem"
        here.save("1", wav(1f))
        here.save("2", wav(1f))
        val out = ByteArrayOutputStream()
        val backup = here.export(sentences, out, "3")
        assertTrue(backup.fileName.startsWith("voice-me-backup-"))

        val there = store()
        val mine = wav(0.5f)
        there.save("2", mine)
        val imported = there.import(ByteArrayInputStream(out.toByteArray()))
        assertEquals(1, imported.added)
        assertEquals(1, imported.kept)
        assertEquals("3", imported.nextId)
        assertArrayEquals("an existing take is never overwritten", mine, there.wavFile("2").readBytes())
        assertEquals("erdem", there.speaker)

        val training = ByteArrayOutputStream().also { here.export(sentences, it, null) }
        val third = store()
        val again = third.import(ByteArrayInputStream(training.toByteArray()))
        assertEquals(2, again.added)
        assertNull(again.nextId)
    }

    @Test
    fun nothingRecordedCannotBeExported() {
        val s = store()
        assertFalse(runCatching { s.export(sentences, ByteArrayOutputStream(), null) }.isSuccess)
    }

    @Test
    fun jsonEscapesAndReadsBack() {
        val name = "Er\"dem\\ \n\tçelik"
        val json = "{\"speaker\":\"${Json.escape(name)}\"}"
        assertEquals(name, Json.string(json, "speaker"))
        assertNull(Json.string(json, "missing"))
    }
}
