import 'dart:async';

import 'package:flutter/material.dart';
import 'package:webview_flutter/webview_flutter.dart';

import '../clax/api.dart';
import 'daemon_cookie.dart';
import 'webview_surface.dart' show noPinchZoom;

/// From this width the question shows as a dialog rather than a bottom
/// sheet.
const questionDialogBreakpoint = 720.0;

/// The shell's inbox focused on question [qid] (`/inbox?q=<qid>`), on the
/// daemon at [base]: the question's own card, answered in the shell.
Uri questionUrl(Uri base, String qid) =>
    base.replace(path: '/inbox', queryParameters: {'q': qid});

/// Whether a question page may show [url] as its document: only the inbox
/// of the daemon at [base].
bool questionPageAllows(Uri base, String url) {
  final u = Uri.tryParse(url);
  return u != null &&
      u.scheme == base.scheme &&
      u.host.toLowerCase() == base.host.toLowerCase() &&
      u.port == base.port &&
      u.userInfo.isEmpty &&
      u.path == '/inbox';
}

/// A view of the shell's question card ([questionUrl]): a WebView
/// ([WebViewQuestionPage]), or a fake in tests. The sheet that is given a
/// page owns it and disposes it.
abstract interface class QuestionPage {
  /// The view, sized by its parent.
  Widget build(BuildContext context);

  /// Loads [url] as the page's document.
  Future<void> load(Uri url);

  void dispose();
}

/// [QuestionPage] over a platform WebView: JavaScript on (the shell is a
/// script), and the top level held to the daemon's inbox
/// ([questionPageAllows]); frames inside the page navigate freely. With
/// [keyCookie], each load first awaits the daemon's key cookie, and a
/// loaded page loads again on a daemon restart ([CookieLoads] with
/// `followRestarts`), whose new token its old one no longer matches.
class WebViewQuestionPage implements QuestionPage {
  WebViewQuestionPage({
    required this.base,
    this.keyCookie,
    WebViewController? controller,
  }) : controller = controller ?? WebViewController() {
    noPinchZoom(this.controller);
    this.controller
      ..setJavaScriptMode(JavaScriptMode.unrestricted)
      ..setNavigationDelegate(
        NavigationDelegate(
          onNavigationRequest: (r) =>
              !r.isMainFrame || questionPageAllows(base, r.url)
              ? NavigationDecision.navigate
              : NavigationDecision.prevent,
        ),
      );
    _loads = CookieLoads(
      keyCookie,
      base,
      load: this.controller.loadRequest,
      reload: this.controller.reload,
      followRestarts: true,
    );
  }

  /// The daemon's base URL (`http://127.0.0.1:<port>`).
  final Uri base;
  final WebViewController controller;

  /// The daemon's key cookie; none for a daemon without
  /// `--require-credential`.
  final DaemonKeyCookie? keyCookie;
  late final CookieLoads _loads;

  @override
  Widget build(BuildContext context) => WebViewWidget(controller: controller);

  @override
  Future<void> load(Uri url) => _loads.load(url);

  @override
  void dispose() => _loads.dispose();
}

/// Shows question [questionId] of the daemon at [base] in the shell's
/// question card, on [page]: a bottom sheet on a phone, a dialog from
/// [questionDialogBreakpoint] wide. It closes by itself once [updates]
/// (the `questions` stream topic's views) shows the question no longer
/// open, and completes as it closes.
Future<void> showQuestionSheet(
  BuildContext context, {
  required QuestionPage page,
  required Uri base,
  required String questionId,
  required Stream<QuestionView> updates,
}) {
  final sheet = QuestionWebSheet(
    page: page,
    url: questionUrl(base, questionId),
    updates: updates,
    questionId: questionId,
  );
  final size = MediaQuery.sizeOf(context);
  if (size.width >= questionDialogBreakpoint) {
    return showDialog<void>(
      context: context,
      builder: (_) => Dialog(
        clipBehavior: Clip.antiAlias,
        child: SizedBox(
          width: 560,
          height: (size.height * 0.8).clamp(320, 760).toDouble(),
          child: sheet,
        ),
      ),
    );
  }
  return showModalBottomSheet<void>(
    context: context,
    isScrollControlled: true,
    useSafeArea: true,
    // The page scrolls; a drag on it must not take the sheet down.
    enableDrag: false,
    clipBehavior: Clip.antiAlias,
    builder: (_) => SizedBox(height: size.height * 0.86, child: sheet),
  );
}

/// The agent's question [questionId], answered in the shell's own question
/// card at [url] on [page]: a header with Close over the page. It closes
/// its route once [updates] shows the question answered, declined or
/// withdrawn, and disposes [page].
class QuestionWebSheet extends StatefulWidget {
  const QuestionWebSheet({
    super.key,
    required this.page,
    required this.url,
    required this.updates,
    required this.questionId,
  });

  final QuestionPage page;
  final Uri url;
  final Stream<QuestionView> updates;
  final String questionId;

  @override
  State<QuestionWebSheet> createState() => _QuestionWebSheetState();
}

class _QuestionWebSheetState extends State<QuestionWebSheet> {
  StreamSubscription<QuestionView>? _sub;
  var _closed = false;

  @override
  void initState() {
    super.initState();
    unawaited(widget.page.load(widget.url));
    _sub = widget.updates.listen((q) {
      if (q.id == widget.questionId && q.status != 'open') _close();
    });
  }

  /// Takes the sheet's route away, on top or not.
  void _close() {
    if (_closed || !mounted) return;
    _closed = true;
    final route = ModalRoute.of(context);
    if (route == null) return;
    final nav = Navigator.of(context);
    if (route.isCurrent) {
      nav.pop();
    } else if (route.isActive) {
      nav.removeRoute(route);
    }
  }

  @override
  void dispose() {
    unawaited(_sub?.cancel());
    widget.page.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Material(
      color: theme.colorScheme.surface,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(20, 8, 8, 4),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    'Clax has a question',
                    style: theme.textTheme.titleMedium,
                  ),
                ),
                IconButton(
                  tooltip: 'Close',
                  icon: const Icon(Icons.close_rounded),
                  onPressed: _close,
                ),
              ],
            ),
          ),
          Expanded(child: widget.page.build(context)),
        ],
      ),
    );
  }
}
