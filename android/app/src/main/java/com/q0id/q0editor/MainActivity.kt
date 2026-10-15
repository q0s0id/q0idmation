package com.q0id.q0editor

import android.content.Context
import android.os.Bundle
import android.os.Process
import android.text.InputType
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import com.google.androidgamesdk.GameActivity

class MainActivity : GameActivity() {
    companion object {
        init {
            System.loadLibrary("q0editor_android")
        }
    }

    private val surfaceOrigin = IntArray(2)
    private lateinit var textSink: TextSink

    private external fun nativeImeCompose(text: String)
    private external fun nativeImeCommit(text: String)
    private external fun nativeImeFinish()
    private external fun nativeImeDelete(before: Int, after: Int)
    private external fun nativeKeyboardCovers(pixels: Int)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // The window spans the whole display. Padding the content view keeps the
        // editor's surface clear of what would hide or swallow touches: the
        // status bar, a button navigation bar and the camera cutout. Gesture
        // navigation only draws a handle, so the surface runs on underneath it.
        //
        // The keyboard is reported rather than padded for: resizing the
        // surface every time it opens would rescale the whole interface.
        WindowCompat.setDecorFitsSystemWindows(window, false)
        val content = mSurfaceView.parent as ViewGroup
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            val clear = insets.getInsets(
                WindowInsetsCompat.Type.tappableElement() or
                    WindowInsetsCompat.Type.displayCutout()
            )
            view.setPadding(clear.left, clear.top, clear.right, clear.bottom)
            val keyboard = insets.getInsets(WindowInsetsCompat.Type.ime()).bottom - clear.bottom
            nativeKeyboardCovers(keyboard.coerceAtLeast(0))
            insets
        }

        textSink = TextSink(this)
        content.addView(textSink, ViewGroup.LayoutParams(1, 1))
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

    override fun onTouchEvent(event: MotionEvent): Boolean {
        return super.onTouchEvent(relativeToSurface(event))
    }

    override fun onGenericMotionEvent(event: MotionEvent): Boolean {
        return super.onGenericMotionEvent(relativeToSurface(event))
    }

    // Events reach the activity in window coordinates, while the native side
    // reads them as positions on its surface.
    private fun relativeToSurface(event: MotionEvent): MotionEvent {
        mSurfaceView.getLocationInWindow(surfaceOrigin)
        event.offsetLocation(-surfaceOrigin[0].toFloat(), -surfaceOrigin[1].toFloat())
        return event
    }

    override fun onDestroy() {
        // GameActivity blocks this thread until the native main function
        // returns, and the editor's event loop never returns. Ending the
        // process is the only way out that does not hang.
        Process.killProcess(Process.myPid())
        super.onDestroy()
    }

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
    }
}
