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

    @Test
    fun `envelope frames total the requested duration`() {
        // MediaMuxer repeats the last inter-sample gap for the final sample, so
        // two frames must sit half a duration apart or the track claims double
        // the note's length.
        val times = envelopeFrameTimesUs(durationUs = 10_611_519L)
        assertEquals(2, times.size)
        assertEquals(0L, times[0])
        assertEquals(5_305_759L, times[1])
        assertEquals(10_611_518L, times[1] * 2)
    }

    @Test
    fun `audio duration counts frames instead of trusting timestamps`() {
        // The recorder writes its own duration at half rate: this take's 583
        // frames carry 13.537 s of audio but the header declares 27.074 s.
        assertEquals(13_537_233L, audioDurationFromFrameCountUs(frames = 583, sampleRate = 44_100))
        assertEquals(0L, audioDurationFromFrameCountUs(frames = 0, sampleRate = 44_100))
        assertEquals(0L, audioDurationFromFrameCountUs(frames = 583, sampleRate = 0))
    }
}
