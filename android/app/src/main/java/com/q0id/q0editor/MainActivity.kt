package com.q0id.q0editor

import android.content.Context
import android.os.Bundle
import android.text.InputType
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import com.q0id.android.Q0Activity

class MainActivity : Q0Activity() {
    companion object {
        init {
            System.loadLibrary("q0editor_android")
        }
    }

    private lateinit var textSink: TextSink

    // Every pixel goes to the canvas: no status bar, and the surface runs
    // past the camera cutout to the edge of the display.
    override val clearsCutout = false
    override val hidesSystemBars = true

    private external fun nativeImeCompose(text: String)
    private external fun nativeImeCommit(text: String)
    private external fun nativeImeFinish()
    private external fun nativeImeDelete(before: Int, after: Int)
    private external fun nativeKeyboardCovers(pixels: Int)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        textSink = TextSink(this)
        (mSurfaceView.parent as ViewGroup).addView(textSink, ViewGroup.LayoutParams(1, 1))
    }

    override fun onKeyboardCovers(pixels: Int) {
        nativeKeyboardCovers(pixels)
    }

    /** Called by the native side whenever one of its text fields gains or loses focus. */
    @Suppress("unused")
    fun setSoftKeyboardVisible(visible: Boolean) {
        runOnUiThread {
            val inputMethods = getSystemService(InputMethodManager::class.java)
            // Android opens the keyboard by itself whenever a window comes up
            // with a text editor in focus, so the sink can take focus only
            // while a field is actually being edited.
            textSink.isFocusable = visible
            textSink.isFocusableInTouchMode = visible
            if (visible) {
                textSink.requestFocus()
                inputMethods.showSoftInput(textSink, 0)
            } else {
                inputMethods.hideSoftInputFromWindow(textSink.windowToken, 0)
            }
        }
    }

    // The native key path loses Enter: it arrives there as a line-break
    // character instead of a key. Sent as typed text it comes out right, for
    // hardware keyboards and on-screen ones alike.
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (isEnter(keyCode)) {
            nativeImeCommit("\n")
            return true
        }
        return super.onKeyDown(keyCode, event)
    }

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean {
        return isEnter(keyCode) || super.onKeyUp(keyCode, event)
    }

    private fun isEnter(keyCode: Int): Boolean =
        keyCode == KeyEvent.KEYCODE_ENTER || keyCode == KeyEvent.KEYCODE_NUMPAD_ENTER

    /**
     * An invisible text target for the input method. The editor draws its own
     * text fields, so this only relays what is typed to the native side.
     */
    private inner class TextSink(context: Context) : View(context) {
        override fun onCheckIsTextEditor(): Boolean = true

        override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
            // The input method never sees the text it produced, so features
            // that go back and rewrite earlier words have to stay off.
            outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_FLAG_NO_FULLSCREEN

            return object : BaseInputConnection(this, false) {
                override fun setComposingText(text: CharSequence, newCursorPosition: Int): Boolean {
                    nativeImeCompose(text.toString())
                    return true
                }

                override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
                    nativeImeCommit(text.toString())
                    return true
                }

                override fun finishComposingText(): Boolean {
                    nativeImeFinish()
                    return true
                }

                override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                    nativeImeDelete(beforeLength, afterLength)
                    return true
                }
            }
        }

        // Many keyboards send digits as key presses instead of text, as do
        // hardware keyboards for everything. The native side gets key presses
        // without the characters they type, so printable ones go over as text
        // while a field is being edited. Enter, Backspace and the arrows carry
        // no printable character and keep going the native way.
        override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
            val typed = typedText(event) ?: return super.onKeyDown(keyCode, event)
            nativeImeCommit(typed)
            return true
        }

        override fun onKeyMultiple(keyCode: Int, repeatCount: Int, event: KeyEvent): Boolean {
            val characters = event.characters
            if (keyCode == KeyEvent.KEYCODE_UNKNOWN && !characters.isNullOrEmpty()) {
                nativeImeCommit(characters)
                return true
            }
            return super.onKeyMultiple(keyCode, repeatCount, event)
        }

        private fun typedText(event: KeyEvent): String? {
            // Ctrl and Alt combinations are shortcuts, not typing.
            if (event.isCtrlPressed || event.isAltPressed || event.isMetaPressed) {
                return null
            }
            val character = event.unicodeChar
            if (character == 0 || character and KeyCharacterMap.COMBINING_ACCENT != 0 ||
                Character.isISOControl(character)
            ) {
                return null
            }
            return String(Character.toChars(character))
        }
    }
}
