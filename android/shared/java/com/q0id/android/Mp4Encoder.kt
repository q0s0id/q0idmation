package com.q0id.android

import android.media.Image
import android.media.MediaCodec
import android.media.MediaCodecInfo.CodecCapabilities
import android.media.MediaFormat
import android.media.MediaMuxer
import java.io.IOException

/**
 * H.264 in an MP4 file through the device's own encoder, fed from native code
 * one NV12 frame at a time: [width] x [height] luma bytes, then a half-size
 * plane of interleaved U and V.
 */
class Mp4Encoder(path: String, private val width: Int, private val height: Int, fps: Int, bitrate: Int) {
    private companion object {
        const val MIME = MediaFormat.MIMETYPE_VIDEO_AVC
        const val WAIT_MICROS = 10_000L
        /** How long the encoder may take to hand back the last frames. */
        const val DRAIN_TRIES = 500
    }

    private val codec = MediaCodec.createEncoderByType(MIME)
    private val muxer: MediaMuxer
    private val info = MediaCodec.BufferInfo()
    private val frameMicros = 1_000_000L / fps.coerceAtLeast(1)
    private var track = -1
    private var frames = 0L

    init {
        val format = MediaFormat.createVideoFormat(MIME, width, height).apply {
            // Flexible YUV lets the encoder lay its planes out as it likes;
            // the Image API below describes that layout.
            setInteger(MediaFormat.KEY_COLOR_FORMAT, CodecCapabilities.COLOR_FormatYUV420Flexible)
            setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
            setInteger(MediaFormat.KEY_FRAME_RATE, fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
        }
        try {
            codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            codec.start()
            muxer = MediaMuxer(path, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
        } catch (error: Exception) {
            codec.release()
            throw error
        }
    }

    @Suppress("unused")
    fun writeFrame(luma: ByteArray, chroma: ByteArray) {
        val index = nextInput()
        val image = codec.getInputImage(index) ?: throw IOException("the encoder offered no input image")
        val planes = image.planes
        copyPlane(planes[0], luma, 0, 1, width, height)
        copyPlane(planes[1], chroma, 0, 2, width / 2, height / 2)
        copyPlane(planes[2], chroma, 1, 2, width / 2, height / 2)
        codec.queueInputBuffer(index, 0, width * height * 3 / 2, frames * frameMicros, 0)
        frames++
        drain(false)
    }

    @Suppress("unused")
    fun finish() {
        try {
            val index = nextInput()
            codec.queueInputBuffer(index, 0, 0, frames * frameMicros, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
            drain(true)
            if (track < 0) {
                throw IOException("the encoder produced no video")
            }
            muxer.stop()
        } finally {
            release()
        }
    }

    /** Gives up on the file, after a failure or a cancelled export. */
    @Suppress("unused")
    fun abort() {
        release()
    }

    private fun release() {
        runCatching { codec.stop() }
        codec.release()
        runCatching { muxer.release() }
    }

    private fun nextInput(): Int {
        while (true) {
            val index = codec.dequeueInputBuffer(WAIT_MICROS)
            if (index >= 0) {
                return index
            }
            // A full encoder frees input buffers only as its output is taken.
            drain(false)
        }
    }

    /**
     * Moves finished output into the file. Before the end it takes only what
     * is ready; at the end it waits for the end-of-stream marker.
     */
    private fun drain(toEnd: Boolean) {
        var tries = 0
        while (true) {
            val index = codec.dequeueOutputBuffer(info, if (toEnd) WAIT_MICROS else 0)
            when {
                index == MediaCodec.INFO_TRY_AGAIN_LATER -> {
                    if (!toEnd) return
                    if (++tries > DRAIN_TRIES) throw IOException("the encoder stopped answering")
                }
                index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    track = muxer.addTrack(codec.outputFormat)
                    muxer.start()
                }
                index >= 0 -> {
                    val data = codec.getOutputBuffer(index)
                    // The format change above already carried the codec setup.
                    val sample = info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0
                    if (data != null && sample && info.size > 0 && track >= 0) {
                        data.position(info.offset)
                        data.limit(info.offset + info.size)
                        muxer.writeSampleData(track, data, info)
                    }
                    codec.releaseOutputBuffer(index, false)
                    if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) return
                }
            }
        }
    }

    /**
     * Writes every [step]th byte of [source], starting at [first], into
     * [plane] as [columns] x [rows] samples, wherever the plane's strides put
     * them.
     */
    private fun copyPlane(plane: Image.Plane, source: ByteArray, first: Int, step: Int, columns: Int, rows: Int) {
        val buffer = plane.buffer
        val rowStride = plane.rowStride
        val pixelStride = plane.pixelStride
        val sourceRow = columns * step
        if (step == 1 && pixelStride == 1) {
            for (row in 0 until rows) {
                buffer.position(row * rowStride)
                buffer.put(source, row * sourceRow, columns)
            }
            return
        }
        for (row in 0 until rows) {
            val target = row * rowStride
            val start = row * sourceRow + first
            for (column in 0 until columns) {
                buffer.put(target + column * pixelStride, source[start + column * step])
            }
        }
    }
}
