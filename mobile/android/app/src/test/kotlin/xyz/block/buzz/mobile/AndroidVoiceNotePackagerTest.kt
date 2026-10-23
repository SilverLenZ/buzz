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
}
