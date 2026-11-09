package com.voiceme.recorder

data class Sentence(val id: String, val text: String)

object Sentences {
    /** Combining dot left by a lowercased `İ`, `|` (a column break), runs of spaces. */
    fun clean(text: String): String =
        text.replace("i̇", "i").replace("İ", "İ").replace('|', ',')
            .trim().split(Regex("\\s+")).joinToString(" ")

    private fun safeId(id: String) =
        id.isNotEmpty() && id.all { it.isLetterOrDigit() || it == '_' || it == '-' }

    /** `id|text` lines; junk, empty texts and repeated ids are skipped. */
    fun parse(csv: String): List<Sentence> {
        val seen = HashSet<String>()
        val out = ArrayList<Sentence>()
        for (line in csv.trimStart('﻿').lineSequence()) {
            val bar = line.indexOf('|')
            if (bar < 0) continue
            val id = line.substring(0, bar).trim()
            val text = clean(line.substring(bar + 1))
            if (safeId(id) && text.isNotEmpty() && seen.add(id)) out.add(Sentence(id, text))
        }
        return out
    }
}
