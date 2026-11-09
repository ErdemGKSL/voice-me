package com.voiceme.recorder

import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.log10
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * Captured samples -> the clip that is saved: mono, 22.05 kHz, silence
 * trimmed, levelled, faded, 16-bit WAV. Same steps and constants as the
 * desktop recorder's `audio.rs`, so both apps produce the same kind of clip.
 */
object Dsp {
    const val TARGET_RATE = 22_050
    const val MIN_SECONDS = 0.8f
    const val MIN_SNR_DB = 30f

    private const val KEEP_SECONDS = 0.20f
    private const val CLIP_PEAK = 0.985f
    private const val QUIET_PEAK = 0.01f
    private const val TARGET_PEAK = 0.89f
    private const val MAX_GAIN = 32.0f
    private const val FADE_SECONDS = 0.008f

    class Quality(val seconds: Float, val peak: Float, val snrDb: Float?, val rawSeconds: Float = seconds) {
        val tooShort get() = seconds < MIN_SECONDS
        val clipped get() = peak >= CLIP_PEAK
        val tooQuiet get() = peak < QUIET_PEAK
        val noisy get() = snrDb != null && snrDb < MIN_SNR_DB
    }

    class Prepared(val clip: FloatArray, val quality: Quality)

    fun peak(s: FloatArray): Float {
        var p = 0f
        for (v in s) p = max(p, abs(v))
        return p
    }

    fun removeDc(s: FloatArray) {
        if (s.isEmpty()) return
        var sum = 0.0
        for (v in s) sum += v
        val mean = (sum / s.size).toFloat()
        for (i in s.indices) s[i] -= mean
    }

    /** Kaiser-free windowed sinc (Blackman) resampler to [TARGET_RATE]. */
    fun resample(input: FloatArray, rate: Int): FloatArray {
        require(rate > 0) { "the microphone reports no sample rate" }
        if (rate == TARGET_RATE || input.isEmpty()) return input
        // scale < 1 when downsampling: the cutoff drops to the target's Nyquist.
        val scale = min(1.0, TARGET_RATE.toDouble() / rate)
        val ratio = rate.toDouble() / TARGET_RATE
        val half = 16.0 / scale // 16 lobes of the target's sinc, in input samples
        val outLength = (input.size / ratio).toInt()
        val out = FloatArray(outLength)
        for (n in 0 until outLength) {
            val t = n * ratio
            val from = max(0, (t - half).toInt() + 1)
            val to = min(input.size - 1, (t + half).toInt())
            var acc = 0.0
            var norm = 0.0
            for (i in from..to) {
                val x = i - t
                val w = x / half // -1..1
                val window = 0.42 + 0.5 * cos(PI * w) + 0.08 * cos(2 * PI * w)
                val arg = PI * scale * x
                val sinc = if (abs(arg) < 1e-9) 1.0 else sin(arg) / arg
                val h = scale * sinc * window
                acc += input[i] * h
                norm += h
            }
            // Dividing by the kernel sum keeps unity gain at DC, edges included.
            out[n] = if (norm > 1e-9) (acc / norm).toFloat() else 0f
        }
        return out
    }

    /**
     * Cut leading and trailing non-speech, keeping [KEEP_SECONDS] of it. A
     * 10 ms frame is speech when its RMS is well above the take's own noise
     * floor and not far below its loudest frame, so a phone that records at
     * -40 dBFS trims the same as one that records at -10 dBFS.
     */
    fun trimSilence(s: FloatArray, rate: Int): FloatArray {
        val frame = max(1, rate / 100)
        val count = s.size / frame
        if (count == 0) return FloatArray(0)
        val levels = FloatArray(count) { f ->
            var sum = 0.0
            for (i in f * frame until (f + 1) * frame) sum += s[i] * s[i]
            sqrt(sum / frame).toFloat()
        }
        val sorted = levels.sortedArray()
        val noise = sorted[count / 10]
        val loudest = sorted[count - 1]
        // Never above half the loudest frame, so a take with no silence in it is kept whole.
        val threshold = max(min(max(noise * 4f, loudest * 0.04f), loudest * 0.5f), 1e-5f)
        val first = levels.indexOfFirst { it >= threshold }
        if (first < 0) return FloatArray(0)
        val last = levels.indexOfLast { it >= threshold }
        val keep = (KEEP_SECONDS * rate).toInt()
        return s.copyOfRange(max(0, first * frame - keep), min(s.size, (last + 1) * frame + keep))
    }

    /** Speech level over room level, in dB; null when the clip is too short. */
    fun snrDb(s: FloatArray, rate: Int): Float? {
        val frame = max(1, rate / 50)
        val count = s.size / frame
        if (count < 20) return null
        val levels = FloatArray(count) { f ->
            var sum = 0.0
            for (i in f * frame until (f + 1) * frame) sum += s[i] * s[i]
            sqrt(sum / frame).toFloat()
        }
        levels.sort()
        val tenth = count / 10
        fun rms(from: Int, to: Int): Double {
            var sum = 0.0
            for (i in from until to) sum += levels[i].toDouble() * levels[i]
            return sqrt(sum / (to - from))
        }
        val noise = max(rms(0, tenth), 1e-6)
        val speech = rms(count - tenth, count)
        return (20 * log10(speech / noise)).toFloat()
    }

    fun normalize(s: FloatArray) {
        val p = peak(s)
        if (p < 1e-6f) return
        val gain = min(TARGET_PEAK / p, MAX_GAIN)
        for (i in s.indices) s[i] *= gain
    }

    fun fadeEdges(s: FloatArray, rate: Int) {
        val length = min((FADE_SECONDS * rate).toInt(), s.size / 2)
        for (n in 0 until length) {
            val gain = (0.5 - 0.5 * cos(PI * n / length)).toFloat()
            s[n] *= gain
            s[s.size - 1 - n] *= gain
        }
    }

    /**
     * DC removed, mono input resampled, trimmed, levelled and faded. The
     * quality is measured before levelling, so "too quiet" and "clipped"
     * describe the microphone, not the gain.
     */
    fun prepare(raw: FloatArray, rate: Int): Prepared {
        val mono = raw.copyOf()
        removeDc(mono)
        val rawPeak = peak(mono)
        val resampled = resample(mono, rate)
        val clip = trimSilence(resampled, TARGET_RATE)
        val q = Quality(clip.size / TARGET_RATE.toFloat(), rawPeak, snrDb(clip, TARGET_RATE), raw.size / rate.toFloat())
        normalize(clip)
        fadeEdges(clip, TARGET_RATE)
        return Prepared(clip, q)
    }

    /** 16-bit mono WAV at [TARGET_RATE]; one quantisation, with rounding. */
    fun wavBytes(s: FloatArray): ByteArray {
        val dataBytes = s.size * 2
        val buf = ByteBuffer.allocate(44 + dataBytes).order(ByteOrder.LITTLE_ENDIAN)
        buf.put("RIFF".toByteArray()).putInt(36 + dataBytes).put("WAVE".toByteArray())
        buf.put("fmt ".toByteArray()).putInt(16).putShort(1).putShort(1)
            .putInt(TARGET_RATE).putInt(TARGET_RATE * 2).putShort(2).putShort(16)
        buf.put("data".toByteArray()).putInt(dataBytes)
        for (v in s) buf.putShort((v.coerceIn(-1f, 1f) * Short.MAX_VALUE).roundToInt().toShort())
        return buf.array()
    }

    /** Seconds of a WAV if it is mono, 22 050 Hz, 16-bit PCM; null otherwise. */
    fun wavSeconds(bytes: ByteArray): Float? {
        if (bytes.size < 44) return null
        val b = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
        if (String(bytes, 0, 4) != "RIFF" || String(bytes, 8, 4) != "WAVE") return null
        val format = b.getShort(20).toInt()
        val channels = b.getShort(22).toInt()
        val rate = b.getInt(24)
        val bits = b.getShort(34).toInt()
        if (format != 1 || channels != 1 || rate != TARGET_RATE || bits != 16) return null
        return (bytes.size - 44) / 2f / TARGET_RATE
    }

    fun toWavStream(s: FloatArray): ByteArrayOutputStream = ByteArrayOutputStream().apply { write(wavBytes(s)) }
}
