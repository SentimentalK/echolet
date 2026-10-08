package com.mainstayx.echolet

/**
 * Pure character-unit (Unicode scalar value) projection of the wire diff,
 * extracted from [WireEvents] so it can be unit-tested on the plain JVM
 * (JUnit, no Android runtime, no org.json).
 *
 * Contract mirrors the shared Rust `src/diff.rs` exactly: `backspaces` pops
 * complete Unicode scalar values (NOT UTF-16 code units: an emoji/CJK
 * extension-B surrogate pair counts as ONE pop), then `suffix` is appended.
 * Kotlin's `StringBuilder` indexes UTF-16 code units, so pops go through
 * `StringBuilder.offsetByCodePoints` (surrogate-aware).
 *
 * `backspaces < 0` (bad value) and `backspaces > scalarCount(buffer)`
 * (corruption) are malformed wire events: they fail loudly instead of
 * silently truncating or crashing the UI.
 */
object CodepointDiffBuffer {
    fun apply(visible: StringBuilder, backspaces: Int, suffix: String) {
        if (backspaces < 0) {
            throw IllegalArgumentException(
                "malformed wire event: negative backspaces ($backspaces)"
            )
        }
        val scalarCount = scalarCount(visible)
        if (backspaces > scalarCount) {
            throw IllegalArgumentException(
                "malformed wire event: backspaces ($backspaces) exceeds visible " +
                    "character-unit count ($scalarCount); refusing silent truncation"
            )
        }
        var popped = 0
        while (popped < backspaces) {
            popLastCodepoint(visible)
            popped++
        }
        visible.append(suffix)
    }

    /** Current Unicode scalar values (Rust `chars()` count), not UTF-16 units. */
    fun scalarCount(visible: CharSequence): Int =
        Character.codePointCount(visible, 0, visible.length)

    /** Removes the last complete Unicode scalar value, surrogate-pair aware. */
    fun popLastCodepoint(visible: StringBuilder) {
        val len = visible.length
        if (len == 0) {
            throw IllegalArgumentException(
                "malformed wire event: backspace on empty visible buffer"
            )
        }
        val start = visible.offsetByCodePoints(len, -1)
        visible.setLength(start)
    }
}
