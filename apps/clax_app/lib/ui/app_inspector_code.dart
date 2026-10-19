/// The app inspector's read-only code view: monospace text with line
/// numbers, scrolling both ways, and a light highlighting of comments,
/// strings, numbers and keywords by file type.
library;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'tokens.g.dart';

/// The language a file's lines are highlighted as.
enum CodeLanguage { html, css, js, json, plain }

/// The [CodeLanguage] of [path], by its extension.
CodeLanguage languageOf(String path) =>
    switch (path.substring(path.lastIndexOf('.') + 1).toLowerCase()) {
      'html' || 'htm' || 'svg' || 'xml' => CodeLanguage.html,
      'css' => CodeLanguage.css,
      'js' || 'mjs' || 'ts' => CodeLanguage.js,
      'json' => CodeLanguage.json,
      _ => CodeLanguage.plain,
    };

/// The kind of one highlighted run of text.
enum TokenKind { text, comment, string, number, keyword, tag }

/// One run of a line: [text] of [kind].
typedef CodeToken = ({String text, TokenKind kind});

const _jsKeywords = {
  'async', 'await', 'break', 'case', 'catch', 'class', 'const', 'continue',
  'default', 'delete', 'do', 'else', 'export', 'extends', 'false', 'finally',
  'for', 'from', 'function', 'if', 'import', 'in', 'instanceof', 'let',
  'new', 'null', 'of', 'return', 'static', 'super', 'switch', 'this',
  'throw', 'true', 'try', 'typeof', 'undefined', 'var', 'void', 'while',
  'yield', //
};

final _patterns = <CodeLanguage, RegExp>{
  // 1 line comment, 2 block comment, 3 string, 4 number, 5 word.
  CodeLanguage.js: RegExp(
    r'(//.*$)|(/\*.*?(?:\*/|$))|("(?:[^"\\]|\\.)*"?|'
    r"'(?:[^'\\]|\\.)*'?|`(?:[^`\\]|\\.)*`?)|(\b\d[\d_.]*\b)|([A-Za-z_$][\w$]*)",
  ),
  // 1 comment, 2 string, 3 number or colour, 4 custom property or at-rule.
  CodeLanguage.css: RegExp(
    r'(/\*.*?(?:\*/|$))|("(?:[^"\\]|\\.)*"?|'
    r"'(?:[^'\\]|\\.)*'?)|(#[0-9a-fA-F]{3,8}\b|\b\d[\d.]*(?:px|rem|em|%|vh|vw|ms|s|fr|deg)?)|(--[\w-]+|@[\w-]+)",
  ),
  // 1 string, 2 number, 3 literal.
  CodeLanguage.json: RegExp(
    r'("(?:[^"\\]|\\.)*"?)|(-?\b\d[\d.eE+-]*)|(\btrue\b|\bfalse\b|\bnull\b)',
  ),
  // 1 comment, 2 attribute value, 3 tag.
  CodeLanguage.html: RegExp(
    r'(<!--.*?(?:-->|$))|(="[^"]*"?|'
    r"='[^']*'?)|(</?[A-Za-z][\w-]*|/?>)",
  ),
};

/// Splits [lines] into highlighted tokens, a list per line. Block comments
/// (`/* */`, `<!-- -->`) and JavaScript template strings carry over from
/// line to line.
List<List<CodeToken>> highlight(List<String> lines, CodeLanguage lang) {
  final pattern = _patterns[lang];
  if (pattern == null) {
    return [
      for (final l in lines) [(text: l, kind: TokenKind.text)],
    ];
  }
  final (opener, closer) = switch (lang) {
    CodeLanguage.html => ('<!--', '-->'),
    CodeLanguage.js || CodeLanguage.css => ('/*', '*/'),
    _ => ('', ''),
  };
  // The run still open at the end of the previous line: what closes it and
  // what it is.
  ({String close, TokenKind kind})? open;
  final out = <List<CodeToken>>[];
  for (final line in lines) {
    final tokens = <CodeToken>[];
    var at = 0;
    if (open != null) {
      final end = line.indexOf(open.close);
      if (end < 0) {
        out.add([(text: line, kind: open.kind)]);
        continue;
      }
      at = end + open.close.length;
      tokens.add((text: line.substring(0, at), kind: open.kind));
      open = null;
    }
    for (final m in pattern.allMatches(line, at)) {
      if (m.start > at) {
        tokens.add((text: line.substring(at, m.start), kind: TokenKind.text));
      }
      final text = m[0]!;
      final kind = switch (lang) {
        CodeLanguage.js when m[1] != null || m[2] != null => TokenKind.comment,
        CodeLanguage.js when m[3] != null => TokenKind.string,
        CodeLanguage.js when m[4] != null => TokenKind.number,
        CodeLanguage.js =>
          _jsKeywords.contains(text) ? TokenKind.keyword : TokenKind.text,
        CodeLanguage.css when m[1] != null => TokenKind.comment,
        CodeLanguage.css when m[2] != null => TokenKind.string,
        CodeLanguage.css when m[3] != null => TokenKind.number,
        CodeLanguage.css => TokenKind.keyword,
        CodeLanguage.json when m[1] != null => TokenKind.string,
        CodeLanguage.json when m[2] != null => TokenKind.number,
        CodeLanguage.json => TokenKind.keyword,
        CodeLanguage.html when m[1] != null => TokenKind.comment,
        CodeLanguage.html when m[2] != null => TokenKind.string,
        CodeLanguage.html => TokenKind.tag,
        CodeLanguage.plain => TokenKind.text,
      };
      if (kind == TokenKind.comment &&
          opener.isNotEmpty &&
          text.startsWith(opener) &&
          !(text.length >= opener.length + closer.length &&
              text.endsWith(closer))) {
        open = (close: closer, kind: TokenKind.comment);
      } else if (lang == CodeLanguage.js &&
          kind == TokenKind.string &&
          text.startsWith('`') &&
          (text.length < 2 || !text.endsWith('`'))) {
        open = (close: '`', kind: TokenKind.string);
      }
      tokens.add((text: text, kind: kind));
      at = m.end;
    }
    if (at < line.length) {
      tokens.add((text: line.substring(at), kind: TokenKind.text));
    }
    out.add(tokens);
  }
  return out;
}

/// A read-only view of [text]: line numbers in a gutter, the lines in
/// monospace highlighted as [language], scrolling sideways for long lines.
class CodeView extends StatefulWidget {
  const CodeView({super.key, required this.text, required this.language});

  final String text;
  final CodeLanguage language;

  @override
  State<CodeView> createState() => _CodeViewState();
}

class _CodeViewState extends State<CodeView> {
  late List<String> _lines;
  late List<List<CodeToken>> _tokens;
  final _horizontal = ScrollController();

  /// Lines past this are shown without highlighting.
  static const _highlightLines = 5000;

  @override
  void initState() {
    super.initState();
    _split();
  }

  @override
  void didUpdateWidget(CodeView old) {
    super.didUpdateWidget(old);
    if (old.text != widget.text || old.language != widget.language) _split();
  }

  @override
  void dispose() {
    _horizontal.dispose();
    super.dispose();
  }

  void _split() {
    _lines = widget.text.replaceAll('\r\n', '\n').split('\n');
    if (_lines.length > 1 && _lines.last.isEmpty) _lines.removeLast();
    final head = _lines.length > _highlightLines
        ? _lines.sublist(0, _highlightLines)
        : _lines;
    _tokens = [
      ...highlight(head, widget.language),
      for (final l in _lines.skip(head.length))
        [(text: l, kind: TokenKind.text)],
    ];
  }

  static Color _color(TokenKind k, ColorScheme s, bool dark) => switch (k) {
    TokenKind.text => s.onSurface,
    TokenKind.comment => s.onSurfaceVariant.withValues(alpha: 0.75),
    TokenKind.string =>
      dark ? const Color(0xFFA5D6A7) : const Color(0xFF2E7D32),
    TokenKind.number =>
      dark ? const Color(0xFFFFCC80) : const Color(0xFFB45309),
    TokenKind.keyword => s.primary,
    TokenKind.tag => s.tertiary,
  };

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final dark = theme.brightness == Brightness.dark;
    final mono = ClaxType.mono(theme.platform);
    final base = TextStyle(
      fontFamily: mono.fontFamily,
      fontFamilyFallback: mono.fontFamilyFallback,
      fontSize: 13,
      height: 1.5,
    );
    final style = base.copyWith(color: scheme.onSurface);
    final scaler = MediaQuery.textScalerOf(context);
    final probe = TextPainter(
      text: TextSpan(text: 'M', style: style),
      textDirection: TextDirection.ltr,
      textScaler: scaler,
    )..layout();
    final charWidth = probe.width;
    final lineHeight = probe.height;
    probe.dispose();
    final digits = '${_lines.length}'.length;
    final gutter = charWidth * digits + 24;
    final longest = _lines.fold<int>(0, (m, l) => l.length > m ? l.length : m);
    return LayoutBuilder(
      builder: (context, box) {
        final width = (gutter + charWidth * (longest + 2) + 16).clamp(
          box.maxWidth,
          double.infinity,
        );
        return Scrollbar(
          controller: _horizontal,
          child: SingleChildScrollView(
            controller: _horizontal,
            scrollDirection: Axis.horizontal,
            child: SizedBox(
              width: width,
              child: SelectionArea(
                child: ListView.builder(
                  padding: const EdgeInsets.symmetric(vertical: 12),
                  itemExtent: lineHeight,
                  itemCount: _lines.length,
                  itemBuilder: (context, i) => Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      SizedBox(
                        width: gutter,
                        child: Padding(
                          padding: const EdgeInsets.only(right: 12),
                          child: SelectionContainer.disabled(
                            child: Text(
                              '${i + 1}',
                              textAlign: TextAlign.right,
                              style: style.copyWith(
                                color: scheme.onSurfaceVariant.withValues(
                                  alpha: 0.6,
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                      Expanded(
                        child: Text.rich(
                          TextSpan(
                            children: [
                              for (final t in _tokens[i])
                                TextSpan(
                                  text: t.text,
                                  style: TextStyle(
                                    color: _color(t.kind, scheme, dark),
                                    fontStyle: t.kind == TokenKind.comment
                                        ? FontStyle.italic
                                        : null,
                                  ),
                                ),
                            ],
                          ),
                          style: style,
                          softWrap: false,
                          maxLines: 1,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// A full page around a [CodeView]: [title] with [subtitle] in the bar and
/// a button copying [text].
class CodePage extends StatelessWidget {
  const CodePage({
    super.key,
    required this.title,
    required this.subtitle,
    required this.text,
    required this.language,
  });

  final String title;
  final String subtitle;
  final String text;
  final CodeLanguage language;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(
        title: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(title, overflow: TextOverflow.ellipsis),
            Text(
              subtitle,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
        actions: [
          IconButton(
            tooltip: 'Copy',
            icon: const Icon(Icons.copy_rounded),
            onPressed: () async {
              await Clipboard.setData(ClipboardData(text: text));
              if (context.mounted) {
                ScaffoldMessenger.of(context)
                    .showSnackBar(const SnackBar(content: Text('Copied')));
              }
            },
          ),
        ],
      ),
      body: CodeView(text: text, language: language),
    );
  }
}
