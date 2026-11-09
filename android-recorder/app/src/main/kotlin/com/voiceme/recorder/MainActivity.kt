package com.voiceme.recorder

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.media.MediaPlayer
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.WindowInsets
import android.view.WindowManager
import android.widget.AdapterView
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.ScrollView
import android.widget.Spinner
import android.widget.Switch
import android.widget.TextView
import java.io.File
import kotlin.concurrent.thread

/**
 * Reads a Turkish sentence at you and saves what you say, one WAV per
 * sentence, in the same format the desktop `voice-me-recorder` uses. The
 * zip it exports (or the backup zip, to carry on elsewhere) is what the
 * Colab notebook trains from.
 */
class MainActivity : Activity() {
    private companion object {
        const val REQ_MIC = 1
        const val REQ_EXPORT = 2
        const val REQ_BACKUP = 3
        const val REQ_IMPORT = 4
        val GREEN = Color.rgb(0x1a, 0x7f, 0x37)
        val RED = Color.rgb(0xcf, 0x22, 0x2e)
        val GREY = Color.rgb(0x57, 0x60, 0x6a)
    }

    private lateinit var store: Store
    private var sentences: List<Sentence> = emptyList()
    private val seconds = HashMap<String, Float>()
    private var index = 0
    private var capture: Capture? = null
    private var player: MediaPlayer? = null
    private var autoAdvance = true
    private var busy = false
    private var mics: List<Mic> = emptyList()
    private var pendingExportName = ""

    private val ui = Handler(Looper.getMainLooper())
    private lateinit var progressText: TextView
    private lateinit var progressBar: ProgressBar
    private lateinit var infoText: TextView
    private lateinit var sentenceText: TextView
    private lateinit var levelBar: ProgressBar
    private lateinit var statusText: TextView
    private lateinit var recordButton: Button
    private lateinit var playButton: Button
    private lateinit var deleteButton: Button
    private lateinit var prevButton: Button
    private lateinit var nextButton: Button
    private lateinit var openButton: Button
    private lateinit var exportButton: Button
    private lateinit var backupButton: Button
    private lateinit var importButton: Button
    private lateinit var speakerEdit: EditText
    private lateinit var micSpinner: Spinner

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        store = Store(File(filesDir, "recordings"))
        sentences = Sentences.parse(assets.open("sentences.csv").bufferedReader().use { it.readText() })
        seconds.putAll(store.recorded())
        index = nextOpenFrom(0) ?: 0
        buildUi()
        refresh()
        say("Kaydet'e bas, cümleyi oku, bitince tekrar bas.", GREY)
    }

    // ---- UI ---------------------------------------------------------------

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    private fun label(text: String, sp: Float = 14f, color: Int = Color.BLACK) =
        TextView(this).apply {
            this.text = text
            textSize = sp
            setTextColor(color)
        }

    private fun button(text: String, onClick: () -> Unit) = Button(this).apply {
        this.text = text
        isAllCaps = false
        setOnClickListener { onClick() }
    }

    private fun row(vararg views: View) = LinearLayout(this).apply {
        orientation = LinearLayout.HORIZONTAL
        for (v in views) addView(v, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
    }

    private fun buildUi() {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(16), dp(16), dp(16))
        }
        progressText = label("", 14f, GREY)
        progressBar = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply { max = 1000 }
        infoText = label("", 13f, GREY)
        sentenceText = label("", 24f).apply {
            setPadding(dp(8), dp(16), dp(8), dp(16))
            gravity = Gravity.CENTER_VERTICAL
            setTextIsSelectable(false)
        }
        levelBar = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            visibility = View.INVISIBLE
        }
        statusText = label("", 15f, GREY).apply { setPadding(0, dp(8), 0, dp(8)) }

        recordButton = button("Kaydet") { toggleRecord() }.apply { textSize = 20f }
        playButton = button("Dinle") { play() }
        deleteButton = button("Sil") { deleteTake() }
        prevButton = button("← Önceki") { goto(index - 1) }
        nextButton = button("Sonraki →") { goto(index + 1) }
        openButton = button("Sıradaki boş cümle") { nextOpen() }

        speakerEdit = EditText(this).apply {
            hint = "Konuşmacı adı"
            setSingleLine()
            setText(if (store.hasSpeaker) store.speaker else "")
        }
        val saveSpeaker = button("Adı kaydet") { saveSpeaker() }

        micSpinner = Spinner(this)
        mics = Mics.list(this)
        micSpinner.adapter = ArrayAdapter(this, android.R.layout.simple_spinner_dropdown_item, mics.map { it.label })
        val saved = store.mic
        micSpinner.setSelection(mics.indexOfFirst { it.key == saved }.coerceAtLeast(0))
        micSpinner.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
            override fun onItemSelected(parent: AdapterView<*>?, view: View?, position: Int, id: Long) {
                store.mic = mics[position].key.ifEmpty { null }
            }

            override fun onNothingSelected(parent: AdapterView<*>?) {}
        }

        val auto = Switch(this).apply {
            text = "Kayıttan sonra otomatik ilerle"
            isChecked = true
            setOnCheckedChangeListener { _, checked -> autoAdvance = checked }
        }
        exportButton = button("Eğitim zip'i") { startExport(false) }
        backupButton = button("Yedekle (devam için)") { startExport(true) }
        importButton = button("İçe aktar") { startImport() }

        root.addView(progressText)
        root.addView(progressBar)
        root.addView(row(speakerEdit, saveSpeaker))
        root.addView(label("Mikrofon", 12f, GREY))
        root.addView(micSpinner)
        root.addView(infoText)
        root.addView(sentenceText, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(220)))
        root.addView(levelBar)
        root.addView(statusText)
        root.addView(recordButton, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(72)))
        root.addView(row(prevButton, playButton, nextButton))
        root.addView(row(deleteButton, openButton))
        root.addView(auto)
        root.addView(label("", 8f))
        root.addView(row(exportButton, backupButton, importButton))
        // Android 15 draws apps edge to edge: keep the content clear of the status bar,
        // the navigation bar, a camera cutout and the keyboard, where the phone puts
        // its own controls.
        val scroll = ScrollView(this).apply {
            addView(root)
            clipToPadding = false
            setOnApplyWindowInsetsListener { v, insets ->
                if (Build.VERSION.SDK_INT >= 30) {
                    val bars = insets.getInsets(
                        WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout() or WindowInsets.Type.ime(),
                    )
                    v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
                } else {
                    ("DEPRECATION")
                    v.setPadding(
                        insets.systemWindowInsetLeft,
                        insets.systemWindowInsetTop,
                        insets.systemWindowInsetRight,
                        insets.systemWindowInsetBottom,
                    )
                }
                insets
            }
        }
        setContentView(scroll)
        scroll.requestApplyInsets()
    }

    private fun say(text: String, color: Int) {
        statusText.text = text
        statusText.setTextColor(color)
    }

    private fun current() = sentences.getOrNull(index)

    private fun refresh() {
        val total = sentences.size
        val done = seconds.size
        val minutes = seconds.values.sum() / 60f
        progressText.text = "$done / $total cümle kaydedildi · ${"%.1f".format(minutes)} dk"
        progressBar.progress = if (total == 0) 0 else (done * 1000L / total).toInt()
        val s = current()
        val has = s != null && seconds.containsKey(s.id)
        infoText.text = "Cümle ${index + 1} / $total · " + if (has) "kaydedildi" else "henüz kayıt yok"
        sentenceText.text = s?.text ?: "Cümle listesi boş."
        val recording = capture != null
        recordButton.text = if (recording) "Durdur" else "Kaydet"
        recordButton.isEnabled = !busy
        playButton.isEnabled = has && !recording
        deleteButton.isEnabled = has && !recording
        prevButton.isEnabled = !recording
        nextButton.isEnabled = !recording
        openButton.isEnabled = !recording
        micSpinner.isEnabled = !recording
        val canExport = !busy && !recording && done > 0
        exportButton.isEnabled = canExport
        backupButton.isEnabled = canExport
        importButton.isEnabled = !busy && !recording
        levelBar.visibility = if (recording) View.VISIBLE else View.INVISIBLE
    }

    // ---- navigation ---------------------------------------------------------

    private fun nextOpenFrom(from: Int): Int? {
        val n = sentences.size
        for (k in 0 until n) {
            val i = (from + k) % n
            if (!seconds.containsKey(sentences[i].id)) return i
        }
        return null
    }

    private fun goto(i: Int) {
        if (capture != null || sentences.isEmpty()) return
        index = i.coerceIn(0, sentences.size - 1)
        stopPlayback()
        refresh()
    }

    private fun nextOpen() {
        val next = nextOpenFrom(index + 1)
        if (next == null) {
            say("Tüm cümleler kaydedildi. Zip'i dışa aktarabilirsin.", GREEN)
            return
        }
        goto(next)
    }

    // ---- recording ----------------------------------------------------------

    private fun toggleRecord() {
        if (busy) return
        if (capture != null) stopRecord() else startRecord()
    }

    private fun startRecord() {
        if (current() == null) return
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQ_MIC)
            return
        }
        stopPlayback()
        val device = mics.getOrNull(micSpinner.selectedItemPosition)?.info
        val c = Capture(this, device)
        val error = c.start()
        if (error != null) {
            say(error, RED)
            return
        }
        capture = c
        say("Kayıt yapılıyor… bitince Durdur.", RED)
        refresh()
        ui.post(object : Runnable {
            override fun run() {
                val live = capture ?: return
                if (!live.running) {
                    stopRecord() // the 30 s limit
                    return
                }
                levelBar.progress = (live.peak * 300).toInt().coerceAtMost(100)
                ui.postDelayed(this, 60)
            }
        })
    }

    private fun stopRecord() {
        val c = capture ?: return
        capture = null
        val raw = c.stop()
        val sentence = current() ?: return
        busy = true
        say("İşleniyor…", GREY)
        refresh()
        thread {
            val result = runCatching {
                val prepared = Dsp.prepare(raw, Capture.RATE)
                val q = prepared.quality
                when {
                    q.tooQuiet -> error(
                        "Ses neredeyse hiç gelmiyor (tepe %.0f dBFS). Mikrofona yaklaş ya da Mikrofon listesinden başka bir mikrofon seç."
                            .format(20 * Math.log10(maxOf(q.peak, 1e-6f).toDouble())),
                    )
                    q.tooShort && q.rawSeconds >= 1.5f -> error(
                        "Konuşma algılanamadı (%.1f sn kayıt var, %.1f sn konuşma bulundu). Mikrofona yaklaş ve tekrar dene."
                            .format(q.rawSeconds, q.seconds),
                    )
                    q.tooShort -> error("Kayıt çok kısa (%.1f sn), tekrar dene.".format(q.rawSeconds))
                }
                store.save(sentence.id, Dsp.wavBytes(prepared.clip))
                q to prepared.clip.size / Dsp.TARGET_RATE.toFloat()
            }
            runOnUiThread {
                busy = false
                result.onSuccess { (q, secs) ->
                    seconds[sentence.id] = secs
                    val snr = q.snrDb?.let { ", SNR %.0f dB".format(it) }.orEmpty()
                    when {
                        q.clipped -> say("Kaydedildi ama ses patlamış (clipping); sesi biraz uzaktan tekrar kaydet.", RED)
                        q.noisy -> say("Kaydedildi ama ortam gürültülü$snr; daha sessiz bir yerde tekrar kaydet.", RED)
                        else -> say("Kaydedildi (%.1f sn$snr).".format(secs), GREEN)
                    }
                    if (autoAdvance && !q.clipped && !q.noisy) {
                        nextOpenFrom(index + 1)?.let { index = it }
                    }
                }
                result.onFailure { say(it.message ?: "Kayıt işlenemedi.", RED) }
                refresh()
            }
        }
    }

    private fun play() {
        val id = current()?.id ?: return
        val file = store.wavFile(id)
        if (!file.isFile) return
        stopPlayback()
        player = MediaPlayer().apply {
            setDataSource(file.absolutePath)
            setOnCompletionListener { stopPlayback() }
            prepare()
            start()
        }
    }

    private fun stopPlayback() {
        player?.let {
            try {
                it.release()
            } catch (_: Exception) {
            }
        }
        player = null
    }

    private fun deleteTake() {
        val id = current()?.id ?: return
        stopPlayback()
        store.delete(id)
        seconds.remove(id)
        say("Kayıt silindi.", GREY)
        refresh()
    }

    private fun saveSpeaker() {
        val name = speakerEdit.text.toString().trim()
        if (name.isEmpty()) return
        store.speaker = name
        say("Konuşmacı: $name", GREEN)
    }

    // ---- export / import ----------------------------------------------------

    private fun startExport(backup: Boolean) {
        saveSpeaker()
        pendingExportName = store.zipName(store.exportable(sentences), backup)
        val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "application/zip"
            putExtra(Intent.EXTRA_TITLE, pendingExportName)
        }
        startActivityForResult(intent, if (backup) REQ_BACKUP else REQ_EXPORT)
    }

    private fun startImport() {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "*/*"
        }
        startActivityForResult(intent, REQ_IMPORT)
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        val uri = data?.data
        if (resultCode != RESULT_OK || uri == null) return
        when (requestCode) {
            REQ_EXPORT -> export(uri, null)
            REQ_BACKUP -> export(uri, current()?.id ?: "")
            REQ_IMPORT -> import(uri)
        }
    }

    private fun export(uri: Uri, nextId: String?) {
        busy = true
        say("Zip hazırlanıyor…", GREY)
        refresh()
        thread {
            val result = runCatching {
                contentResolver.openOutputStream(uri)!!.use { store.export(sentences, it, nextId?.ifEmpty { null }) }
            }
            runOnUiThread {
                busy = false
                result.onSuccess {
                    val skipped = if (it.skipped > 0) " (${it.skipped} uygunsuz kayıt atlandı)" else ""
                    say("${it.count} cümle, ${"%.1f".format(it.seconds / 60)} dk → ${it.fileName}$skipped", GREEN)
                }
                result.onFailure { say("Dışa aktarılamadı: ${it.message}", RED) }
                refresh()
            }
        }
    }

    private fun import(uri: Uri) {
        busy = true
        say("İçe aktarılıyor…", GREY)
        refresh()
        thread {
            val result = runCatching { contentResolver.openInputStream(uri)!!.use { store.import(it) } }
            runOnUiThread {
                busy = false
                result.onSuccess {
                    seconds.clear()
                    seconds.putAll(store.recorded())
                    if (store.hasSpeaker) speakerEdit.setText(store.speaker)
                    val resume = it.nextId?.let { id -> sentences.indexOfFirst { s -> s.id == id } }?.takeIf { i -> i >= 0 }
                    index = resume ?: nextOpenFrom(0) ?: 0
                    say("${it.added} kayıt eklendi, ${it.kept} zaten vardı, ${it.skipped} atlandı.", GREEN)
                }
                result.onFailure { say("İçe aktarılamadı: ${it.message}", RED) }
                refresh()
            }
        }
    }

    // ---- lifecycle ----------------------------------------------------------

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == REQ_MIC) {
            if (grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) {
                startRecord()
            } else {
                say("Mikrofon izni verilmedi; Ayarlar'dan izin ver.", RED)
            }
        }
    }

    override fun onPause() {
        super.onPause()
        // A take cut off by leaving the app is discarded, not saved half-said.
        capture?.let {
            it.stop()
            capture = null
            say("Uygulamadan çıkıldığı için kayıt iptal edildi.", RED)
            refresh()
        }
        stopPlayback()
    }
}
