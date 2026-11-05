package app.mirrorleaf

import android.graphics.Bitmap
import android.graphics.ColorSpace
import android.graphics.ImageDecoder
import android.os.Build
import android.util.Log
import androidx.annotation.RequiresApi
import app.mirrorleaf.engine.DecodedPixels
import app.mirrorleaf.engine.PhotoDecoder
import java.nio.ByteBuffer
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * Decodes HEIC and AVIF photos for the engine, which can't in Rust (plan §5.5): Android's
 * ImageDecoder turns them upright, scales while decoding, and converts to sRGB. JPEG, PNG and
 * WebP never come here. Needs Android 9; earlier versions can't add HEIC photos.
 */
object PhotoDecoding : PhotoDecoder {
    override fun decode(bytes: ByteArray, maxEdge: UInt): DecodedPixels? {
        if (Build.VERSION.SDK_INT < 28) return null
        return try {
            decode28(bytes, maxEdge.toInt())
        } catch (e: Exception) {
            Log.w(MirrorleafApp.TAG, "couldn't decode a photo: ${e.message}")
            null
        }
    }

    @RequiresApi(28)
    private fun decode28(bytes: ByteArray, maxEdge: Int): DecodedPixels {
        val source = ImageDecoder.createSource(ByteBuffer.wrap(bytes))
        val decoded = ImageDecoder.decodeBitmap(source) { decoder, info, _ ->
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
            decoder.setTargetColorSpace(ColorSpace.get(ColorSpace.Named.SRGB))
            val (w, h) = info.size.width to info.size.height
            val scale = min(1.0, maxEdge.toDouble() / max(w, h))
            if (scale < 1.0) {
                decoder.setTargetSize(max(1, (w * scale).roundToInt()), max(1, (h * scale).roundToInt()))
            }
        }
        val bitmap =
            if (decoded.config == Bitmap.Config.ARGB_8888) decoded
            else decoded.copy(Bitmap.Config.ARGB_8888, false)
        // ARGB_8888 is stored as R, G, B, A bytes. Rows may be padded; the engine wants them packed.
        val (w, h, row) = Triple(bitmap.width, bitmap.height, bitmap.rowBytes)
        val raw = ByteBuffer.allocate(row * h).also { bitmap.copyPixelsToBuffer(it) }.array()
        val rgba =
            if (row == w * 4) raw
            else ByteArray(w * h * 4).also { packed ->
                for (y in 0 until h) System.arraycopy(raw, y * row, packed, y * w * 4, w * 4)
            }
        return DecodedPixels(w.toUInt(), h.toUInt(), rgba)
    }
}
