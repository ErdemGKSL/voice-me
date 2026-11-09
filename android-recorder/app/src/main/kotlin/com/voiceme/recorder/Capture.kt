package com.voiceme.recorder

import android.annotation.SuppressLint
import android.content.Context
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.MediaRecorder
import kotlin.math.abs
import kotlin.math.max

/** A microphone the user can pick. Bluetooth is left out: it records at 8-16 kHz. */
class Mic(val key: String, val label: String, val info: AudioDeviceInfo?)

object Mics {
    fun list(context: Context): List<Mic> {
        val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
        val devices = audio.getDevices(AudioManager.GET_DEVICES_INPUTS).filter {
            it.type != AudioDeviceInfo.TYPE_BLUETOOTH_SCO && it.type != AudioDeviceInfo.TYPE_BLUETOOTH_A2DP
        }
        return listOf(Mic("", "Sistem varsayılanı", null)) + devices.map {
            Mic("${it.type}|${it.productName}", "${it.productName} (${typeName(it.type)})", it)
        }
    }

    private fun typeName(type: Int) = when (type) {
        AudioDeviceInfo.TYPE_BUILTIN_MIC -> "dahili"
        AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_WIRED_HEADPHONES -> "kulaklık"
        AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET -> "USB"
        else -> "tip $type"
    }
}

/**
 * One take from the microphone, 48 kHz mono. The least-processed source the
 * phone offers is used (UNPROCESSED, else VOICE_RECOGNITION): no automatic
 * gain, noise suppression or echo cancellation, which would colour a
 * training set.
 */
class Capture(private val context: Context, private val device: AudioDeviceInfo?) {
    companion object {
        const val RATE = 48_000
        const val MAX_SECONDS = 30
    }

    @Volatile var peak = 0f
        private set
    @Volatile var running = false
        private set

    private var record: AudioRecord? = null
    private var thread: Thread? = null
    private var buffer = FloatArray(RATE * 10)
    private var length = 0

    val seconds get() = length / RATE.toFloat()

    /** Null when started, else the reason it could not be. */
    @SuppressLint("MissingPermission")
    fun start(): String? {
        val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
        val unprocessed = audio.getProperty(AudioManager.PROPERTY_SUPPORT_AUDIO_SOURCE_UNPROCESSED) == "true"
        val sources = listOf(
            if (unprocessed) MediaRecorder.AudioSource.UNPROCESSED else MediaRecorder.AudioSource.VOICE_RECOGNITION,
            MediaRecorder.AudioSource.MIC,
        )
        val minimum = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        if (minimum <= 0) return "Bu telefon 48 kHz kaydı desteklemiyor."
        var created: AudioRecord? = null
        for (source in sources) {
            val candidate = try {
                AudioRecord(source, RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, minimum * 4)
            } catch (e: Exception) {
                continue
            }
            if (candidate.state == AudioRecord.STATE_INITIALIZED) {
                created = candidate
                break
            }
            candidate.release()
        }
        val rec = created ?: return "Mikrofon açılamadı (başka bir uygulama kullanıyor olabilir)."
        if (device != null) rec.setPreferredDevice(device)
        buffer = FloatArray(RATE * 10)
        length = 0
        peak = 0f
        record = rec
        try {
            rec.startRecording()
        } catch (e: Exception) {
            rec.release()
            record = null
            return "Kayıt başlatılamadı: ${e.message}"
        }
        running = true
        thread = Thread {
            val chunk = ShortArray(RATE / 20)
            while (running) {
                val n = rec.read(chunk, 0, chunk.size)
                if (n <= 0) continue
                var p = 0f
                synchronized(this) {
                    if (length + n > buffer.size) buffer = buffer.copyOf(max(buffer.size * 2, length + n))
                    for (i in 0 until n) {
                        val v = chunk[i] / 32768f
                        buffer[length + i] = v
                        p = max(p, abs(v))
                    }
                    length += n
                }
                peak = p
                if (length >= RATE * MAX_SECONDS) running = false
            }
        }.also { it.start() }
        return null
    }

    /** Stop and return the samples. */
    fun stop(): FloatArray {
        running = false
        thread?.join(500)
        record?.let {
            try {
                it.stop()
            } catch (_: Exception) {
            }
            it.release()
        }
        record = null
        return synchronized(this) { buffer.copyOf(length) }
    }
}
