package com.q0id.android

import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.os.Process
import android.provider.OpenableColumns
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.ViewGroup
import android.view.WindowManager
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import androidx.core.content.IntentCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.google.androidgamesdk.GameActivity
import java.io.File
import java.io.IOException
import java.io.OutputStream
import java.security.MessageDigest
import java.util.concurrent.Executors

/**
 * The activity q0editor and q0player both run in.
 *
 * Besides fitting the native surface between the system bars, it gives the
 * native side what Android keeps on this side of JNI: the system document
 * picker, files that other apps hand over, and the Back button.
 *
 * Native code works on plain files, so every document is used through a
 * private copy. A document picked or received is copied in before the native
 * side hears of it; whenever the native side reports that it wrote a copy,
 * the copy goes back out to the document it stands for, which is how saving
 * edits files in place wherever they live.
 */
abstract class Q0Activity : GameActivity() {
    companion object {
        /** The tag of documents other apps send, as opposed to picked ones. */
        const val EXTERNAL = 0
        /** Carries the tag when q0editor hands q0player a preview. */
        const val EXTRA_TAG = "com.q0id.android.extra.TAG"
        const val PLAYER_PACKAGE = "com.q0id.q0player"

        private const val REQUEST_OPEN = 0x0501
        private const val REQUEST_CREATE = 0x0502
    }

    private val surfaceOrigin = IntArray(2)

    /** Copies run here, one at a time and in the order they were asked for. */
    private val files = Executors.newSingleThreadExecutor()

    /** Copies the native side has finished writing. Only touched on [files]. */
    private val written = HashSet<String>()

    /** Which copy stands for which document, kept across launches. */
    private val links by lazy { getSharedPreferences("q0-documents", MODE_PRIVATE) }

    private var pickTag = EXTERNAL
    private var createdFor: String? = null

    private external fun nativeDocumentOpened(tag: Int, path: String)
    private external fun nativeDocumentFailed(tag: Int, message: String)
    private external fun nativeStatus(message: String)
    private external fun nativeBack()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // The window spans the whole display. Padding the content view keeps the
        // native surface clear of what would hide or swallow touches: the
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
            onKeyboardCovers(keyboard.coerceAtLeast(0))
            insets
        }

        // A recreated activity would see its launch intent a second time.
        if (savedInstanceState == null) {
            receive(intent)
        }
    }

    /** How many pixels at the bottom of the surface the soft keyboard covers. */
    protected open fun onKeyboardCovers(pixels: Int) {}

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        receive(intent)
    }

    /** Opens what another app sent: "Open with", "Share" and q0editor previews. */
    private fun receive(intent: Intent?) {
        val documents = when (intent?.action) {
            Intent.ACTION_VIEW, Intent.ACTION_EDIT -> listOfNotNull(intent.data)
            Intent.ACTION_SEND -> listOfNotNull(
                IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)
            )
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)
                    .orEmpty()
            else -> emptyList()
        }
        val tag = intent?.getIntExtra(EXTRA_TAG, EXTERNAL) ?: EXTERNAL
        for (uri in documents) {
            files.execute { open(tag, uri) }
        }
    }

    /** Native side: shows the system document picker. */
    @Suppress("unused")
    fun pickDocument(tag: Int, mimeTypes: Array<String>) {
        runOnUiThread {
            pickTag = tag
            // Extensions such as .q1s have no MIME type, so the picker cannot
            // filter for them. Only all-known lists narrow it down.
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType(mimeTypes.singleOrNull() ?: "*/*")
            if (mimeTypes.size > 1) {
                intent.putExtra(Intent.EXTRA_MIME_TYPES, mimeTypes)
            }
            start(intent, REQUEST_OPEN) {
                nativeDocumentFailed(tag, "this device has no document picker")
            }
        }
    }

    /**
     * Native side: asks where a copy of the file being written at [path]
     * should live. It is copied there once both the choice is made and
     * [documentWritten] reports the file complete, and after every later write.
     */
    @Suppress("unused")
    fun createDocument(path: String, mimeType: String) {
        runOnUiThread {
            createdFor = path
            val intent = Intent(Intent.ACTION_CREATE_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType(mimeType)
                .putExtra(Intent.EXTRA_TITLE, File(path).name)
            start(intent, REQUEST_CREATE) {
                nativeStatus("this device has no document picker; the file stays inside the app")
            }
        }
    }

    /** Native side: [path] was written in full. */
    @Suppress("unused")
    fun documentWritten(path: String) {
        files.execute {
            written.add(path)
            val document = linkOf(path)
            if (document != null) {
                copyOut(path, document)
            } else if (File(path).parentFile?.parentFile == File(filesDir, "opened")) {
                nativeStatus(
                    "saved inside the app only: the app this file came from does not take " +
                        "changes back. Use Save As to keep it somewhere else"
                )
            }
        }
    }

    /**
     * Native side: an export finished at [path]. Its copy goes where one was
     * chosen for it, or the person is asked where now; a file left inside the
     * app would be out of everyone's reach.
     */
    @Suppress("unused")
    fun exportFinished(path: String, mimeType: String) {
        files.execute {
            written.add(path)
            val document = linkOf(path)
            if (document != null) {
                copyOut(path, document)
            } else {
                createDocument(path, mimeType)
            }
        }
    }

    /** Native side: plays a snapshot in q0player. [path] must be in cache/preview. */
    @Suppress("unused")
    fun openInPlayer(path: String, tag: Int) {
        runOnUiThread {
            try {
                val uri = FileProvider.getUriForFile(this, "$packageName.files", File(path))
                val intent = Intent(Intent.ACTION_VIEW)
                    .setDataAndType(uri, "application/octet-stream")
                    .setPackage(PLAYER_PACKAGE)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    .putExtra(EXTRA_TAG, tag)
                start(intent, null) { nativeStatus("install q0player to preview movies") }
            } catch (error: IllegalArgumentException) {
                nativeStatus("could not share the preview with q0player: ${error.message}")
            }
        }
    }

    /** Native side: Back with nothing left to close goes home, like other apps. */
    @Suppress("unused")
    fun leave() {
        runOnUiThread { moveTaskToBack(true) }
    }

    @Suppress("unused")
    fun keepScreenOn(on: Boolean) {
        runOnUiThread {
            if (on) {
                window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
    }

    @Suppress("unused")
    fun immersive(on: Boolean) {
        runOnUiThread {
            val bars = WindowCompat.getInsetsController(window, window.decorView)
            if (on) {
                bars.systemBarsBehavior =
                    WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                bars.hide(WindowInsetsCompat.Type.systemBars())
            } else {
                bars.show(WindowInsetsCompat.Type.systemBars())
            }
        }
    }

    @Deprecated("The activity result API cannot vary the MIME type per request")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        val document = data?.data.takeIf { resultCode == RESULT_OK }
        when (requestCode) {
            REQUEST_OPEN -> {
                val tag = pickTag
                pickTag = EXTERNAL
                if (document == null) {
                    nativeDocumentFailed(tag, "")
                } else {
                    keepAccess(document, data!!.flags)
                    files.execute { open(tag, document) }
                }
            }
            REQUEST_CREATE -> {
                val path = createdFor ?: return
                createdFor = null
                if (document == null) {
                    nativeStatus("kept inside the app only")
                } else {
                    keepAccess(document, data!!.flags)
                    files.execute {
                        link(path, document)
                        if (path in written) {
                            copyOut(path, document)
                        }
                    }
                }
            }
        }
    }

    private fun start(intent: Intent, request: Int?, missing: () -> Unit) {
        try {
            if (request == null) {
                startActivity(intent)
            } else {
                @Suppress("DEPRECATION")
                startActivityForResult(intent, request)
            }
        } catch (_: ActivityNotFoundException) {
            missing()
        }
    }

    /** Lets saving reach a picked document after the app restarts, too. */
    private fun keepAccess(document: Uri, flags: Int) {
        val access = flags and
            (Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        try {
            contentResolver.takePersistableUriPermission(document, access)
        } catch (_: SecurityException) {
            // Not every provider offers lasting access; this session still has it.
        }
    }

    /** Copies [document] in and hands the copy to the native side. Runs on [files]. */
    private fun open(tag: Int, document: Uri) {
        val name = displayName(document)
        try {
            val copy = copyOf(document, name)
            val partial = File(copy.path + ".part")
            val source = contentResolver.openInputStream(document)
                ?: throw IOException("the app holding it returned nothing")
            source.use { input -> partial.outputStream().use { input.copyTo(it) } }
            if (!partial.renameTo(copy)) {
                throw IOException("could not move the copy into place")
            }
            // Saving writes back only where that is allowed at all.
            if (canWrite(document)) {
                link(copy.path, document)
            } else {
                unlink(copy.path)
            }
            nativeDocumentOpened(tag, copy.path)
        } catch (error: Exception) {
            nativeDocumentFailed(tag, "could not open $name: ${describe(error)}")
        }
    }

    /**
     * Where the private copy of [document] lives. The same document always
     * gets the same copy, so opening it again does not pile up duplicates.
     */
    private fun copyOf(document: Uri, name: String): File {
        val key = MessageDigest.getInstance("SHA-256")
            .digest(document.toString().toByteArray())
            .take(6)
            .joinToString("") { "%02x".format(it) }
        val folder = File(File(filesDir, "opened"), key)
        if (!folder.isDirectory && !folder.mkdirs()) {
            throw IOException("no room for a private copy")
        }
        return File(folder, fileName(document, name))
    }

    /** A safe file name, with an extension if the document's name lacks one. */
    private fun fileName(document: Uri, name: String): String {
        val safe = name.replace('/', '_').replace('\u0000', '_').trim().ifEmpty { "document" }
        if (safe.contains('.')) {
            return safe
        }
        val extension = contentResolver.getType(document)
            ?.let { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) }
        return if (extension == null) safe else "$safe.$extension"
    }

    private fun canWrite(document: Uri): Boolean =
        checkUriPermission(
            document,
            Process.myPid(),
            Process.myUid(),
            Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        ) == PackageManager.PERMISSION_GRANTED

    /** Copies the file at [path] over [document]. Runs on [files]. */
    private fun copyOut(path: String, document: Uri) {
        try {
            openTruncated(document).use { sink -> File(path).inputStream().use { it.copyTo(sink) } }
        } catch (error: Exception) {
            nativeStatus(
                "saved inside the app, but ${displayName(document)} could not be updated: " +
                    describe(error)
            )
        }
    }

    /**
     * Opens [document] for a full rewrite. Plain "w" is not tried: some
     * providers leave the old tail in place after a shorter write with it.
     */
    private fun openTruncated(document: Uri): OutputStream {
        for (mode in arrayOf("wt", "rwt")) {
            try {
                contentResolver.openOutputStream(document, mode)?.let { return it }
            } catch (_: Exception) {
                // Try the next mode.
            }
        }
        throw IOException("the app holding it does not allow changes")
    }

    private fun link(path: String, document: Uri) {
        links.edit().putString(path, document.toString()).apply()
    }

    private fun unlink(path: String) {
        links.edit().remove(path).apply()
    }

    private fun linkOf(path: String): Uri? = links.getString(path, null)?.let(Uri::parse)

    private fun displayName(document: Uri): String {
        val queried = try {
            contentResolver.query(document, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                ?.use { if (it.moveToFirst()) it.getString(0) else null }
        } catch (_: Exception) {
            null
        }
        return queried ?: document.lastPathSegment?.substringAfterLast('/') ?: "document"
    }

    private fun describe(error: Exception): String = error.message ?: error.javaClass.simpleName

    // Back goes to the native side, which closes whatever is open and calls
    // [leave] when nothing is. Letting the activity finish instead would end
    // the process: the native event loop cannot restart inside it.
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (keyCode == KeyEvent.KEYCODE_BACK) {
            if (event.repeatCount == 0) {
                nativeBack()
            }
            return true
        }
        return super.onKeyDown(keyCode, event)
    }

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean {
        return keyCode == KeyEvent.KEYCODE_BACK || super.onKeyUp(keyCode, event)
    }

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
        // returns, and the native event loop never returns. Ending the process
        // is the only way out that does not hang.
        Process.killProcess(Process.myPid())
        super.onDestroy()
    }
}
