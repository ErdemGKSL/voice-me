package com.voiceme.recorder

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SentencesTest {
    @Test
    fun linesBecomeSentencesAndJunkIsSkipped() {
        val csv = "﻿00001|Merhaba dünya.\n\nnobar\n00002|  \n00001|again\n../x|bad id\n00003|İyi  kalpli\n"
        assertEquals(
            listOf(Sentence("00001", "Merhaba dünya."), Sentence("00003", "İyi kalpli")),
            Sentences.parse(csv),
        )
    }

    @Test
    fun aStrayCombiningDotAndPipesAreCleaned() {
        assertEquals("o iyi a,b", Sentences.clean("o i̇yi a|b"))
    }

    @Test
    fun theBundledListIsTheOrderedTwentyThousandPlusShortLines() {
        val text = java.io.File("src/main/assets/sentences.csv").readText()
        val all = Sentences.parse(text)
        assertEquals(20_248, all.size)
        assertTrue(all.take(8).count { it.id.startsWith("k") } >= 2)
    }
}
