package dev.empathic.clax_app

import android.graphics.Bitmap
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.StandardMethodCodec
import java.io.ByteArrayInputStream
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicLong

/**
 * The app's scheme and the `ClaxHost` channel in its WebViews, over the
 * method channels `dev.empathic.clax/scheme` (attaching WebViews, on the
 * main thread) and `dev.empathic.clax/scheme-worker` (the scheme's work,
 * on a background task queue, which the app's scheme worker isolate takes:
 * no request of the scheme touches the main thread or the UI isolate).
 *
 * The app's origins are `https://app.clax.invalid` (the shell) and
 * `https://<id>.c.clax.invalid` (an artifact's content); a WebView answers
 * requests to those hosts once Dart attaches it (a request to one of them
 * on any scheme but https is answered 403 here and never reaches Dart):
 *
 * - `scheme`: Dart → native `attach {webViewId}` (answers whether that
 *   WebView exists; it must already have webview_flutter's client, which
 *   this one wraps) and `detach {webViewId}`. webview_flutter's
 *   `setNavigationDelegate` sets a new client, so Dart attaches again after
 *   each one: attaching an attached WebView wraps the client it has now if
 *   that is not this host's.
 * - `scheme-worker`: the native side never calls Dart; it queues events,
 *   and `next` answers those queued since the last `next` (waiting until
 *   there is one): `{event: request, webViewId, reqId, url, method,
 *   headers, mainFrame, navigation?}` for every request to the app's
 *   hosts; `{event: channel, webViewId, message}` for a `ClaxHost` message
 *   from the shell's main frame; `{event: committed, webViewId, url,
 *   navigation?}` when a main-frame navigation commits; `{event: refused,
 *   webViewId, url}` when a main-frame request to the shell's host is not
 *   for a shell document (a path under `/a/`, spec §4.2), which is
 *   answered 403 here and never reaches Dart. `respond {reqId, status,
 *   headers, body}` answers a request; `eval {webViewId, js}` runs a script
 *   in the WebView's main frame (on the main thread, as a WebView takes
 *   it; answers whether it ran); `release` answers a waiting `next` with
 *   null.
 *
 * The shell's fixed files are answered natively once the core has
 * answered them ([ShellCache]): such a request never reaches Dart. Dart's
 * `cacheAt {dir}` on `scheme` names where the cache keeps them.
 *
 * Every answer passes [response], which drops each `Access-Control-*`
 * header (spec §4.3) and answers a 3xx, which a WebResourceResponse cannot
 * carry, as a 502 that says so.
 *
 * `shouldInterceptRequest` runs on a WebView IO thread: it queues the
 * request there (answering a waiting `next` from that thread) and waits for
 * Dart's answer at most [ANSWER_CAP_SECONDS], then answers 504. A request
 * for the main frame (`isForMainFrame`) takes the WebView's next navigation
 * number, which is kept with its URL (a newer one replaces it);
 * `onPageStarted` for that URL reports the number with the commit, any
 * other URL none.
 *
 * It is made once per engine ([EngineChannels]): the WebViews whose clients
 * it wraps, and the requests it holds for them, live as long as the engine,
 * not as long as an activity.
 */
class SchemeHost(
    messenger: BinaryMessenger,
    private val webView: (Long) -> WebView?,
) {
    private val channel = MethodChannel(messenger, "dev.empathic.clax/scheme")
    private val work = MethodChannel(
        messenger,
        "dev.empathic.clax/scheme-worker",
        StandardMethodCodec.INSTANCE,
        messenger.makeBackgroundTaskQueue(),
    )
    private val main = Handler(Looper.getMainLooper())

    /** Guards [events] and [parked], which every thread here touches. */
    private val lock = Any()

    /** The events no `next` has taken yet, oldest first. */
    private val events = ArrayList<Map<String, Any?>>()

    /** The `next` waiting for an event. */
    private var parked: MethodChannel.Result? = null
    private val pending = ConcurrentHashMap<Long, CompletableFuture<WebResourceResponse>>()
    private val cache = ShellCache()

    /** The URL of each request whose answer the cache may keep, by request. */
    private val keeping = ConcurrentHashMap<Long, String>()
    private val nextRequest = AtomicLong()

    /**
     * An attached WebView's state: [inner] is the main thread's; the
     * numbering, which IO threads take, is guarded by the state itself.
     */
    private inner class Attached(val id: Long, val view: WebView, var inner: WebViewClient) {
        var lastNavigation = 0L

        /** The latest main-frame request's number and URL. */
        var kept: Pair<Long, String>? = null
        val client = Client(this)

        /** Whether it is still attached: read off the main thread. */
        @Volatile var live = true
    }

    private val attached = HashMap<Long, Attached>()

    init {
        channel.setMethodCallHandler { call, result ->
            val id = call.argument<Number>("webViewId")?.toLong()
            when (call.method) {
                "attach" -> result.success(id != null && attach(id))
                "detach" -> {
                    id?.let { detach(it) }
                    result.success(null)
                }
                "cacheAt" -> {
                    call.argument<String>("dir")?.let { cache.at(java.io.File(it)) }
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
        // On the background task queue.
        work.setMethodCallHandler { call, result ->
            when (call.method) {
                "next" -> next(result)
                "release" -> {
                    val waiting = synchronized(lock) { parked.also { parked = null } }
                    waiting?.success(null)
                    result.success(null)
                }
                "respond" -> {
                    respond(
                        call.argument<Number>("reqId")?.toLong(),
                        call.argument<Number>("status")?.toInt() ?: 500,
                        call.argument<Map<String, String>>("headers") ?: emptyMap(),
                        call.argument<ByteArray>("body") ?: ByteArray(0),
                    )
                    result.success(null)
                }
                "eval" -> eval(
                    call.argument<Number>("webViewId")?.toLong(),
                    call.argument<String>("js"),
                    result,
                )
                else -> result.notImplemented()
            }
        }
    }

    /**
     * Queues event [name] for the worker's `next`, answering a waiting one
     * at once (from this thread: a reply may be sent from any thread).
     */
    private fun send(name: String, args: Map<String, Any?>) {
        val event = args + ("event" to name)
        val waiting = synchronized(lock) {
            val p = parked
            if (p == null) events.add(event) else parked = null
            p
        }
        waiting?.success(listOf(event))
    }

    /**
     * Answers [result] with the queued events, or keeps it until there is
     * one; a `next` that comes while another waits answers that one empty.
     */
    private fun next(result: MethodChannel.Result) {
        var taken: List<Map<String, Any?>>? = null
        var before: MethodChannel.Result? = null
        synchronized(lock) {
            if (events.isNotEmpty()) {
                taken = ArrayList(events)
                events.clear()
            } else {
                before = parked
                parked = result
            }
        }
        before?.success(emptyList<Any>())
        taken?.let { result.success(it) }
    }

    /** Runs [js] in WebView [id]'s main frame, on the main thread. */
    private fun eval(id: Long?, js: String?, result: MethodChannel.Result) {
        if (id == null || js == null) return result.success(false)
        main.post {
            val state = attached[id]
            if (state == null) {
                result.success(false)
            } else {
                state.view.evaluateJavascript(js) { result.success(true) }
            }
        }
    }

    private fun attach(id: Long): Boolean {
        attached[id]?.let { state ->
            // A client set since (setNavigationDelegate): wrap it instead.
            val now = state.view.webViewClient
            if (now !== state.client) {
                state.inner = now
                state.view.webViewClient = state.client
            }
            return true
        }
        val view = webView(id) ?: return false
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) return false
        val state = Attached(id, view, view.webViewClient)
        attached[id] = state
        view.webViewClient = state.client
        // Injected only into frames of the shell's origin; a message from a
        // frame that is not the main one is dropped too.
        WebViewCompat.addWebMessageListener(view, "ClaxHost", setOf(SHELL_ORIGIN)) {
                _: WebView, message: WebMessageCompat, _: Uri, isMainFrame: Boolean, _: JavaScriptReplyProxy ->
            val text = message.data
            if (isMainFrame && text != null && attached[id] === state) {
                send("channel", mapOf("webViewId" to id, "message" to text))
            }
        }
        return true
    }

    private fun detach(id: Long) {
        val state = attached.remove(id) ?: return
        state.live = false
        if (state.view.webViewClient === state.client) state.view.webViewClient = state.inner
        WebViewCompat.removeWebMessageListener(state.view, "ClaxHost")
    }

    private fun respond(reqId: Long?, status: Int, headers: Map<String, String>, body: ByteArray) {
        reqId?.let { keeping.remove(it) }?.let { cache.keep(it, status, headers, body) }
        val future = reqId?.let { pending.remove(it) } ?: return
        future.complete(response(status, headers, body))
    }

    /** Called on a WebView IO thread: blocks until Dart answers. */
    private fun intercept(state: Attached, request: WebResourceRequest): WebResourceResponse? {
        val url = request.url
        if (!isAppHost(url.host)) return null
        // The app's origins are https only: the same hosts on another
        // scheme (http) are answered here and never reach Dart.
        if (url.scheme?.lowercase() != "https") {
            return response(403, mapOf("Content-Type" to "text/plain"), "refused".toByteArray())
        }
        if (request.isForMainFrame && url.host == SHELL_HOST && url.path?.startsWith("/a/") != true) {
            if (state.live) send("refused", mapOf("webViewId" to state.id, "url" to url.toString()))
            return response(403, mapOf("Content-Type" to "text/plain"), "refused".toByteArray())
        }
        cache.get(url, request.method, request.requestHeaders)?.let {
            return response(it.status, it.headers, it.body)
        }
        val reqId = nextRequest.incrementAndGet()
        val future = CompletableFuture<WebResourceResponse>()
        pending[reqId] = future
        if (ShellCache.cacheable(url, request.method, request.requestHeaders)) {
            keeping[reqId] = url.toString()
        }
        val args = mutableMapOf<String, Any?>(
            "webViewId" to state.id,
            "reqId" to reqId,
            "url" to url.toString(),
            "method" to request.method,
            "headers" to request.requestHeaders,
            "mainFrame" to request.isForMainFrame,
        )
        if (request.isForMainFrame) {
            synchronized(state) {
                state.lastNavigation += 1
                state.kept = state.lastNavigation to url.toString()
                args["navigation"] = state.lastNavigation
            }
        }
        send("request", args)
        return try {
            future.get(ANSWER_CAP_SECONDS, TimeUnit.SECONDS)
        } catch (e: TimeoutException) {
            pending.remove(reqId)
            keeping.remove(reqId)
            response(504, mapOf("Content-Type" to "text/plain"), "no answer".toByteArray())
        } catch (e: Exception) {
            pending.remove(reqId)
            response(500, mapOf("Content-Type" to "text/plain"), "failed".toByteArray())
        }
    }

    private fun committed(state: Attached, url: String) {
        if (attached[state.id] !== state) return
        val args = mutableMapOf<String, Any?>("webViewId" to state.id, "url" to url)
        synchronized(state) { state.kept }?.let { (n, keptUrl) -> if (keptUrl == url) args["navigation"] = n }
        send("committed", args)
    }

    /**
     * webview_flutter's client with the app's hosts answered and commits
     * reported; every other call is the inner client's.
     */
    private inner class Client(private val state: Attached) : WebViewClient() {
        private val inner get() = state.inner

        override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? =
            intercept(state, request) ?: inner.shouldInterceptRequest(view, request)

        override fun onPageStarted(view: WebView, url: String, favicon: Bitmap?) {
            committed(state, url)
            inner.onPageStarted(view, url, favicon)
        }

        override fun onPageFinished(view: WebView, url: String) = inner.onPageFinished(view, url)

        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean =
            inner.shouldOverrideUrlLoading(view, request)

        @Deprecated("Deprecated in Java")
        override fun shouldOverrideUrlLoading(view: WebView, url: String): Boolean =
            @Suppress("DEPRECATION") inner.shouldOverrideUrlLoading(view, url)

        override fun onLoadResource(view: WebView, url: String) = inner.onLoadResource(view, url)

        override fun onPageCommitVisible(view: WebView, url: String) = inner.onPageCommitVisible(view, url)

        override fun onReceivedError(
            view: WebView,
            request: WebResourceRequest,
            error: android.webkit.WebResourceError,
        ) = inner.onReceivedError(view, request, error)

        @Deprecated("Deprecated in Java")
        override fun onReceivedError(view: WebView, errorCode: Int, description: String?, failingUrl: String?) =
            @Suppress("DEPRECATION") inner.onReceivedError(view, errorCode, description, failingUrl)

        override fun onReceivedHttpError(
            view: WebView,
            request: WebResourceRequest,
            errorResponse: WebResourceResponse,
        ) = inner.onReceivedHttpError(view, request, errorResponse)

        override fun onReceivedSslError(
            view: WebView,
            handler: android.webkit.SslErrorHandler,
            error: android.net.http.SslError,
        ) = inner.onReceivedSslError(view, handler, error)

        override fun onReceivedHttpAuthRequest(
            view: WebView,
            handler: android.webkit.HttpAuthHandler,
            host: String,
            realm: String,
        ) = inner.onReceivedHttpAuthRequest(view, handler, host, realm)

        override fun onReceivedClientCertRequest(view: WebView, request: android.webkit.ClientCertRequest) =
            inner.onReceivedClientCertRequest(view, request)

        override fun doUpdateVisitedHistory(view: WebView, url: String, isReload: Boolean) =
            inner.doUpdateVisitedHistory(view, url, isReload)

        override fun onFormResubmission(view: WebView, dontResend: android.os.Message, resend: android.os.Message) =
            inner.onFormResubmission(view, dontResend, resend)

        override fun onScaleChanged(view: WebView, oldScale: Float, newScale: Float) =
            inner.onScaleChanged(view, oldScale, newScale)

        override fun onUnhandledKeyEvent(view: WebView, event: android.view.KeyEvent) =
            inner.onUnhandledKeyEvent(view, event)

        override fun shouldOverrideKeyEvent(view: WebView, event: android.view.KeyEvent): Boolean =
            inner.shouldOverrideKeyEvent(view, event)

        override fun onRenderProcessGone(view: WebView, detail: android.webkit.RenderProcessGoneDetail): Boolean =
            inner.onRenderProcessGone(view, detail)

        override fun onReceivedLoginRequest(view: WebView, realm: String, account: String?, args: String) =
            inner.onReceivedLoginRequest(view, realm, account, args)

        override fun onSafeBrowsingHit(
            view: WebView,
            request: WebResourceRequest,
            threatType: Int,
            callback: android.webkit.SafeBrowsingResponse,
        ) = inner.onSafeBrowsingHit(view, request, threatType, callback)
    }

    companion object {
        const val SHELL_ORIGIN = "https://app.clax.invalid"
        const val SHELL_HOST = "app.clax.invalid"
        private const val CONTENT_SUFFIX = ".c.clax.invalid"
        private const val ANSWER_CAP_SECONDS = 30L

        /** The shell's host, or an artifact's content host. */
        fun isAppHost(host: String?): Boolean =
            host == SHELL_HOST || (host != null && host.endsWith(CONTENT_SUFFIX) &&
                host.length > CONTENT_SUFFIX.length && !host.dropLast(CONTENT_SUFFIX.length).contains('.'))

        /**
         * [status] with [headers] and [body] as a WebView takes it: the
         * content type split into MIME type and charset, and no
         * `Access-Control-*` header. WebResourceResponse takes no 3xx
         * status: one (or a status outside 100..599) is answered as a 502
         * whose body says why.
         */
        fun response(status: Int, headers: Map<String, String>, body: ByteArray): WebResourceResponse {
            if (status in 300..399 || status !in 100..599) {
                return response(
                    502,
                    mapOf("Content-Type" to "text/plain; charset=utf-8", "Cache-Control" to "no-store"),
                    redirectRefusal(status).toByteArray(),
                )
            }
            val kept = headers.filterKeys { !it.lowercase().startsWith("access-control-") }
            val type = kept.entries.firstOrNull { it.key.equals("content-type", ignoreCase = true) }?.value
            val mime = type?.substringBefore(';')?.trim()?.ifEmpty { null } ?: "application/octet-stream"
            val charset = type?.split(';')?.drop(1)?.map { it.trim() }
                ?.firstOrNull { it.startsWith("charset=", ignoreCase = true) }
                ?.substringAfter('=')?.trim('"')
            return WebResourceResponse(mime, charset, status, reason(status), kept, ByteArrayInputStream(body))
        }

        /** The body of the 502 that stands for a [status] the WebView does not take. */
        fun redirectRefusal(status: Int): String =
            "502 Bad Gateway: the app answered $status, a redirect or a status a WebView here does not take"


        private fun reason(status: Int): String = when (status) {
            200 -> "OK"
            204 -> "No Content"
            206 -> "Partial Content"
            400 -> "Bad Request"
            403 -> "Forbidden"
            404 -> "Not Found"
            405 -> "Method Not Allowed"
            500 -> "Internal Server Error"
            502 -> "Bad Gateway"
            504 -> "Gateway Timeout"
            else -> "Status $status"
        }
    }
}
