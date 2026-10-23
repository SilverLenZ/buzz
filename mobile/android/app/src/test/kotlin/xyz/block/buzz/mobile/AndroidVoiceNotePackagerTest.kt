package xyz.block.buzz.mobile

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class AndroidVoiceNotePackagerTest {
    @Test
    fun `voice note envelope stays within the encoder SD baseline`() {
        val envelope = voiceNoteVideoEnvelope

        assertEquals("video/avc", envelope.mimeType)
        // 176x144 is Android's documented SD resolution and the minimum frame
        // size vendor media profiles (notably Samsung's) advertise. Smaller
        // envelopes fail to configure on those encoders, which left recordings
        // impossible to send.
        assertEquals(176, envelope.width)
        assertEquals(144, envelope.height)
        assertEquals(0, envelope.width % 16)
        assertEquals(0, envelope.height % 16)
        assertTrue(envelope.bitRate >= 64_000, "below vendor minimum bit rate")
        assertTrue(envelope.frameRate >= 12, "below documented SD frame rate")
    }

    @Test
    fun `envelope duration follows the samples when the header doubles it`() {
        // Observed on a Samsung S22 Ultra: the recorder's m4a declared 21.548 s
        // for 10.774 s of AAC. Sizing the envelope from the header made every
        // player advertise 21.5 s and then idle in silence for the difference.
        assertEquals(
            10_774_000L,
            envelopeDurationUs(declaredUs = 21_548_000L, measuredUs = 10_774_000L),
        )
    }

    @Test
    fun `envelope duration agrees with a consistent header`() {
        assertEquals(
            12_771_000L,
            envelopeDurationUs(declaredUs = 12_771_000L, measuredUs = 12_771_000L),
        )
    }

    @Test
    fun `envelope duration falls back to the header when samples cannot be walked`() {
        assertEquals(
            5_000_000L,
            envelopeDurationUs(declaredUs = 5_000_000L, measuredUs = 0L),
        )
    }
}
