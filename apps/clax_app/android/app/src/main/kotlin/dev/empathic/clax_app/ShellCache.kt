package dev.empathic.clax_app

import android.net.Uri
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicLong

/**
 * The shell's fixed files, answered natively once the core has answered
 * them: a `GET` to
 * the shell's host under `/_clax/` with neither `Range` nor
 * `Service-Worker`, which the core answered 200 with `Cache-Control: …
 * immutable` (the shell's content-hashed bundles and the transport at its
 * version, in a release core), is kept with its status, headers and body,
 * and the same URL is answered from here after, on the WebView's request
 * thread, without Dart or the core. The bytes and headers are the core's
 * own, and a URL whose bytes could change is never kept (a debug core
 * marks none immutable).
 *
 * Kept answers are written under [at] (one file each, named by a hash of
 * the URL: a line of JSON `{url, status, headers}`, then the body) and read
 * back in the background at the next launch. Past [capBytes] the cache is
 * emptied and starts again. Every method may be called from any thread.
 */
class ShellCache(private val capBytes: Long = 8L shl 20) {
    class Entry(val status: Int, val headers: Map<String, String>, val body: ByteArray)

    private val entries = ConcurrentHashMap<String, Entry>()
    private val io = Executors.newSingleThreadExecutor { r -> Thread(r, "clax shell cache") }

    @Volatile private var dir: File? = null
    private val bytes = AtomicLong()

    /** Keeps answers under [at] from now on, and reads those kept there. */
    fun at(at: File) {
        io.execute {
            if (dir != null) return@execute
            at.mkdirs()
            dir = at
            for (f in at.listFiles() ?: emptyArray()) read(f)?.let { (url, e) ->
                if (entries.putIfAbsent(url, e) == null) bytes.addAndGet(e.body.size.toLong())
            }
        }
    }

    /** The kept answer to a request for [url], if this request may take one. */
    fun get(url: Uri, method: String, headers: Map<String, String>): Entry? =
        if (cacheable(url, method, headers)) entries[url.toString()] else null

    /** Keeps the core's answer to a request [cacheable] said may be kept. */
    fun keep(url: String, status: Int, headers: Map<String, String>, body: ByteArray) {
        if (status != 200 || !immutable(headers) || entries.containsKey(url)) return
        val e = Entry(status, headers, body)
        if (bytes.get() + body.size > capBytes) clear()
        if (entries.putIfAbsent(url, e) != null) return
        bytes.addAndGet(body.size.toLong())
        io.execute { write(url, e) }
    }

    private fun clear() {
        entries.clear()
        bytes.set(0)
        io.execute { dir?.listFiles()?.forEach { it.delete() } }
    }

    private fun write(url: String, e: Entry) {
        val d = dir ?: return
        val head = JSONObject()
            .put("url", url)
            .put("status", e.status)
            .put("headers", JSONObject(e.headers as Map<*, *>))
            .toString()
        val name = name(url)
        val tmp = File(d, "$name.tmp")
        try {
            tmp.outputStream().use { out ->
                out.write(head.toByteArray())
                out.write('\n'.code)
                out.write(e.body)
            }
            tmp.renameTo(File(d, name))
        } catch (_: Exception) {
            tmp.delete()
        }
    }

    private fun read(f: File): Pair<String, Entry>? {
        if (f.name.endsWith(".tmp")) {
            f.delete()
            return null
        }
        return try {
            val all = f.readBytes()
            val nl = all.indexOf('\n'.code.toByte())
            val head = JSONObject(String(all, 0, nl))
            val h = head.getJSONObject("headers")
            val headers = h.keys().asSequence().associateWith { h.getString(it) }
            val url = head.getString("url")
            if (name(url) != f.name || !immutable(headers)) null
            else url to Entry(head.getInt("status"), headers, all.copyOfRange(nl + 1, all.size))
        } catch (_: Exception) {
            f.delete()
            null
        }
    }

    companion object {
        /** Whether a request may be answered from, or its answer kept in, the cache. */
        fun cacheable(url: Uri, method: String, headers: Map<String, String>): Boolean =
            method == "GET" && url.host == SchemeHost.SHELL_HOST &&
                url.path?.startsWith("/_clax/") == true &&
                headers.keys.none { it.equals("range", true) || it.equals("service-worker", true) }

        fun immutable(headers: Map<String, String>): Boolean =
            headers.entries.any { (k, v) -> k.equals("cache-control", true) && v.contains("immutable") }

        private fun name(url: String): String =
            MessageDigest.getInstance("SHA-256").digest(url.toByteArray())
                .joinToString("") { "%02x".format(it) }
    }
}
