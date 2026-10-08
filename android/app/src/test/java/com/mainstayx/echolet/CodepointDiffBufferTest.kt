package com.mainstayx.echolet

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

/**
 * Plain-JVM JUnit tests (no Android runtime) of the character-unit contract
 * the wire diff must satisfy: `backspaces` pops Unicode SCALAR values exactly
 * like Rust `src/diff.rs` `chars()` semantics — NOT UTF-16 code units — then
 * `suffix` is appended.
 */
class CodepointDiffBufferTest {
    @Test
    fun bmp_chinese_correction_matches_char_units() {
        val visible = StringBuilder("我们非常喜")
        CodepointDiffBuffer.apply(visible, 4, "爱你。")
        assertEquals("我爱你。", visible.toString())
    }

    @Test
    fun ascii_suffix_correction() {
        // One pop removes the wrong-character 'r'; "hello wor" -> "hello wor
        // 'r'-pop -> "hello wo", then "rld." completes the word. Rust parity:
        // backspaces pops exactly one char (scalar) each.
        val visible = StringBuilder("hello wor")
        CodepointDiffBuffer.apply(visible, 1, "rld.")
        assertEquals("hello world.", visible.toString())

        // Two pops remove TWO scalars, exactly like Rust chars(): 'r' then 'o'.
        val two = StringBuilder("hello wor")
        CodepointDiffBuffer.apply(two, 2, "rld.")
        assertEquals("hello wrld.", two.toString())
    }

    @Test
    fun surrogate_pairs_pop_complete_scalars() {
        // '🤖' is ONE scalar (two UTF-16 units); a naive UTF-16-unit pop of 1
        // would leave a dangling low surrogate. Scalars: s a y 🤖 🈚 今 天 → 7.
        // Rust parity: 2 scalar pops remove 天 and 今 (NOT the two halves of a
        // surrogate pair), leaving 🈚 visible.
        val visible = StringBuilder("say🤖🈚今天")
        CodepointDiffBuffer.apply(visible, 2, "明天！")
        assertEquals("say🤖🈚明天！", visible.toString())

        // Popping the emoji+letter tail removes the WHOLE surrogate scalar:
        // 'b' is last (pop 1 suffices), popping 2 removes 🤖+'b' together.
        val seen = StringBuilder("a🤖b")
        CodepointDiffBuffer.apply(seen, 2, "x")
        assertEquals("ax", seen.toString())
    }

    @Test
    fun zero_backspaces_and_empty_suffix_are_no_op_appends() {
        val visible = StringBuilder("keep")
        CodepointDiffBuffer.apply(visible, 0, "+right")
        assertEquals("keep+right", visible.toString())
        CodepointDiffBuffer.apply(visible, 0, "")
        assertEquals("keep+right", visible.toString())
    }

    @Test
    fun backspaces_exceeding_scalars_fail_not_truncate() {
        val visible = StringBuilder("ab")
        assertThrows(IllegalArgumentException::class.java) {
            CodepointDiffBuffer.apply(visible, 3, "")
        }
        assertEquals("ab", visible.toString())
    }

    @Test
    fun negative_backspaces_fail() {
        val visible = StringBuilder("ab")
        assertThrows(IllegalArgumentException::class.java) {
            CodepointDiffBuffer.apply(visible, -1, "x")
        }
        assertEquals("ab", visible.toString())
    }

    @Test
    fun scalarCount_counts_scalars_not_utf16_units() {
        assertEquals(5, CodepointDiffBuffer.scalarCount(StringBuilder("🤖🈚abc")))
        assertEquals(3, CodepointDiffBuffer.scalarCount(StringBuilder("abc")))
        assertEquals(0, CodepointDiffBuffer.scalarCount(StringBuilder()))
    }

    @Test
    fun mixed_correction_round_trips_exactly_like_rust_chars() {
        // Rust diff parity: pop '好','🈚','!' (3 scalars incl. one pair) and
        // rewrite the tail with a NEW supplementary pair.
        val visible = StringBuilder("hi 你好🈚!")
        CodepointDiffBuffer.apply(visible, 3, " 好 🌍")
        assertEquals("hi 你 好 🌍", visible.toString())
    }
}
