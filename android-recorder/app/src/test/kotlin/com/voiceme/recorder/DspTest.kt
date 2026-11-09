package com.voiceme.recorder

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.sin
import kotlin.math.sqrt

class DspTest {
    private fun tone(seconds: Float, rate: Int, amplitude: Float, hz: Double = 1000.0) =
        FloatArray((seconds * rate).toInt()) { (amplitude * sin(2 * PI * hz * it / rate)).toFloat() }

    private fun rms(s: FloatArray, from: Int = 0, to: Int = s.size): Double =
        sqrt((from until to).sumOf { s[it].toDouble() * s[it] } / (to - from))

    @Test
    fun resamplingKeepsLengthAndLevelOfATone() {
        val out = Dsp.resample(tone(1f, 48_000, 0.5f), 48_000)
        assertTrue("length ${out.size}", abs(out.size - Dsp.TARGET_RATE) < 5)
        // Away from the edges a 0.5 sine has RMS 0.5/sqrt(2).
        val level = rms(out, 2000, out.size - 2000)
        assertEquals(0.5 / sqrt(2.0), level, 0.01)
    }

    @Test
    fun resamplingKeepsDcAtUnityGainAndSkipsMatchingRates() {
        val out = Dsp.resample(FloatArray(48_000) { 0.25f }, 48_000)
        assertEquals(0.25f, out[10], 1e-4f)
        assertEquals(0.25f, out[out.size - 1], 1e-4f)
        val same = FloatArray(10)
        assertTrue(Dsp.resample(same, Dsp.TARGET_RATE) === same)
    }

    @Test
    fun frequenciesAboveTheTargetNyquistAreRemoved() {
        // 15 kHz is above 11.025 kHz: it must not alias into the band.
        val out = Dsp.resample(tone(1f, 48_000, 0.5f, 15_000.0), 48_000)
        assertTrue(rms(out, 2000, out.size - 2000) < 0.01)
    }

    @Test
    fun silenceIsTrimmedButAMarginStays() {
        val rate = Dsp.TARGET_RATE
        val clip = FloatArray(rate) + tone(1f, rate, 0.5f) + FloatArray(rate)
        val trimmed = Dsp.trimSilence(clip, rate)
        val expected = (1 + 2 * 0.2) * rate
        assertTrue("${trimmed.size}", abs(trimmed.size - expected) < 0.05 * expected)
        assertEquals(0, Dsp.trimSilence(FloatArray(100), rate).size)
    }

    @Test
    fun takesAreJudgedByLengthLevelAndNoise() {
        val rate = Dsp.TARGET_RATE
        val speech = tone(0.5f, rate, 0.5f)
        // A room: noise at `floor` amplitude around half a second of speech.
        fun build(floor: Float, speech: FloatArray = tone(0.5f, rate, 0.5f)): FloatArray {
            var seed = 12345
            fun noise() = FloatArray(rate / 2) {
                seed = seed * 1103515245 + 12345
                floor * ((seed ushr 8) % 2001 - 1000) / 1000f
            }
            return noise() + speech + noise()
        }
        val quiet = Dsp.prepare(build(0.001f), rate).quality
        val noisy = Dsp.prepare(build(0.05f), rate).quality
        assertTrue("${quiet.snrDb}", !quiet.noisy)
        assertTrue("${noisy.snrDb}", noisy.noisy)
        assertTrue(Dsp.prepare(tone(0.3f, rate, 0.5f), rate).quality.tooShort)
        assertTrue(Dsp.prepare(tone(2f, rate, 0.004f), rate).quality.tooQuiet)
        assertTrue(Dsp.prepare(tone(2f, rate, 1.0f), rate).quality.clipped)
        assertNull(Dsp.snrDb(FloatArray(10), rate))
        assertEquals(speech.size, tone(0.5f, rate, 0.5f).size)
    }

    @Test
    fun aQuietPhoneMicrophoneIsNotMistakenForSilence() {
        // Android's UNPROCESSED source records speech around -35 dBFS: a 2 s
        // sentence at 0.02 amplitude over a very quiet room must be kept whole.
        val rate = 48_000
        var seed = 99
        fun noise(n: Int) = FloatArray(n) {
            seed = seed * 1103515245 + 12345
            0.0004f * ((seed ushr 8) % 2001 - 1000) / 1000f
        }
        val take = noise(rate) + tone(2f, rate, 0.02f) + noise(rate)
        val prepared = Dsp.prepare(take, rate)
        val q = prepared.quality
        assertTrue("seconds ${q.seconds}", !q.tooShort && q.seconds in 2.2f..2.8f)
        assertTrue(!q.tooQuiet && !q.noisy && !q.clipped)
        assertTrue("levelled to ${Dsp.peak(prepared.clip)}", Dsp.peak(prepared.clip) > 0.5f)
        assertEquals(4f, q.rawSeconds, 0.01f)
    }

    @Test
    fun preparedClipsAreLevelledAndFaded() {
        val rate = Dsp.TARGET_RATE
        val clip = Dsp.prepare(tone(1.5f, rate, 0.3f).map { it + 0.1f }.toFloatArray(), rate).clip
        assertEquals(0.89f, Dsp.peak(clip), 0.02f)
        assertEquals(0f, clip[0], 1e-3f)
        assertEquals(0f, clip[clip.size - 1], 1e-3f)
    }

    @Test
    fun wavIsSixteenBitMonoAtTheTrainingRate() {
        val bytes = Dsp.wavBytes(tone(0.5f, Dsp.TARGET_RATE, 0.5f))
        assertEquals(0.5f, Dsp.wavSeconds(bytes)!!, 0.001f)
        assertNull(Dsp.wavSeconds(bytes.copyOf(30)))
        val wrongRate = bytes.copyOf().also {
            it[24] = 0x44
            it[25] = 0xAC.toByte()
            it[26] = 0
            it[27] = 0
        } // 44100 Hz
        assertNull(Dsp.wavSeconds(wrongRate))
        assertNotNull(Dsp.wavSeconds(bytes))
    }
}
