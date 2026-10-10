package com.mainstayx.echolet

import android.graphics.Canvas
import android.graphics.ColorFilter
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PixelFormat
import android.graphics.drawable.Drawable

/**
 * Small stroke-drawn keyboard glyphs (chevron, backspace) drawn in a 24-unit
 * view box, so the keyboard needs no font glyphs or XML resources and matches
 * the thin-line look of the desktop panel.
 */
class KeyIconDrawable(
    private val kind: Kind,
    private val sizePx: Int,
    strokePx: Float,
    color: Int,
) : Drawable() {

    enum class Kind { CHEVRON_UP, CHEVRON_DOWN, BACKSPACE }

    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = strokePx
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
        this.color = color
    }

    override fun getIntrinsicWidth(): Int = sizePx
    override fun getIntrinsicHeight(): Int = sizePx

    override fun draw(canvas: Canvas) {
        val b = bounds
        val side = minOf(b.width(), b.height()).toFloat()
        val s = side / 24f
        val ox = b.left + (b.width() - side) / 2f
        val oy = b.top + (b.height() - side) / 2f
        fun x(v: Float) = ox + v * s
        fun y(v: Float) = oy + v * s

        val path = Path()
        when (kind) {
            Kind.CHEVRON_UP -> {
                path.moveTo(x(6f), y(15f)); path.lineTo(x(12f), y(9f)); path.lineTo(x(18f), y(15f))
            }
            Kind.CHEVRON_DOWN -> {
                path.moveTo(x(6f), y(9f)); path.lineTo(x(12f), y(15f)); path.lineTo(x(18f), y(9f))
            }
            Kind.BACKSPACE -> {
                // Key outline with a pointed left edge.
                path.moveTo(x(9f), y(5f))
                path.lineTo(x(19f), y(5f))
                path.quadTo(x(21f), y(5f), x(21f), y(7f))
                path.lineTo(x(21f), y(17f))
                path.quadTo(x(21f), y(19f), x(19f), y(19f))
                path.lineTo(x(9f), y(19f))
                path.lineTo(x(2.5f), y(12f))
                path.close()
                // The "x" mark.
                path.moveTo(x(11.5f), y(9.5f)); path.lineTo(x(16.5f), y(14.5f))
                path.moveTo(x(16.5f), y(9.5f)); path.lineTo(x(11.5f), y(14.5f))
            }
        }
        canvas.drawPath(path, paint)
    }

    override fun setAlpha(alpha: Int) {
        paint.alpha = alpha
        invalidateSelf()
    }

    override fun setColorFilter(colorFilter: ColorFilter?) {
        paint.colorFilter = colorFilter
        invalidateSelf()
    }

    @Deprecated("Deprecated in Java")
    override fun getOpacity(): Int = PixelFormat.TRANSLUCENT
}
