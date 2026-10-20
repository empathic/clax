/// An app's launcher icon: the `icon.svg` among its files, a square
/// full-bleed design in a small, safe subset of SVG.
///
/// The core is authoritative: every publish, from every client, runs its
/// rules (`clax_core::icon::problem`, crates/clax-core/src/icon.rs) and
/// refuses a bad icon with 400 `invalid_icon`. [iconProblem] is the agent's
/// early check of the same rules when it writes the file, with the same
/// codes and sentences; the vectors in crates/clax-core/tests/fixtures/icons
/// are run by both (test/agent/icon_fixtures_test.dart), so the two cannot
/// drift. [cleanIcon] is the reader's, which drops what the subset leaves
/// out (an older app or a hand edit may carry it) and refuses what cannot
/// be drawn.
library;

import 'dart:convert';

import 'package:xml/xml.dart';

/// The path of an app's icon among its files.
const appIconPath = 'icon.svg';

/// The most UTF-8 bytes of an icon.
const maxIconBytes = 32 * 1024;

/// The elements an icon may use: shapes, paths, groups, gradients, clips,
/// masks, references within the file, and text for a single glyph.
const iconElements = {
  'svg', 'g', 'defs', 'title', 'desc', //
  'path', 'rect', 'circle', 'ellipse', 'line', 'polyline', 'polygon',
  'linearGradient', 'radialGradient', 'stop', 'clipPath', 'mask', 'use',
  'symbol', 'text', 'tspan',
};

/// The most `<use>` copies an icon may draw, every expansion counted.
const maxIconUses = 64;

/// The deepest a `<use>` may reach through other `<use>`s.
const maxIconUseDepth = 3;

/// The most levels of elements an icon may nest, its root `<svg>` the
/// first: far below what a recursive walk of the tree (the core's parser,
/// drawing it, [cleanIcon]'s, `toXmlString`) can take on a phone's stack.
/// It is read from the text before anything parses it.
const maxIconDepth = 64;

/// Why an icon is not usable: [code] names the rule broken, as the core's
/// `invalid_icon` error does (`too_large`, `not_xml`, `doctype`, `nesting`,
/// `not_svg`, `view_box`, `element`, `event_attribute`, `external_href`,
/// `filter`, `external_url`, `style`, `use_too_many`, `use_too_deep`), and
/// [message] is the sentence for the model to act on.
typedef IconProblem = ({String code, String message});

/// Why [svg] is not a usable icon; null when it is one. The rules run in
/// this order and the first broken one is named: size, nesting at most
/// [maxIconDepth] deep (read from the text, as the core reads it before its
/// parser), well-formed XML, no DOCTYPE, the core parser's stricter XML
/// ([_strictProblem]), an `<svg>` root, a square `viewBox`, then each element (the root's descendants in
/// document order, the root last) by its name and its attributes, then the
/// `<use>` expansion.
IconProblem? iconProblem(String svg) {
  final bytes = utf8.encode(svg).length;
  if (bytes > maxIconBytes) {
    return (
      code: 'too_large',
      message:
          'icon.svg is $bytes bytes; an icon may be at most 32 KB '
          '($maxIconBytes bytes): simplify its paths.',
    );
  }
  if (_tooDeep(svg)) {
    return (
      code: 'nesting',
      message:
          'icon.svg nests its elements more than $maxIconDepth deep: flatten '
          'its groups.',
    );
  }
  final XmlDocument doc;
  try {
    doc = XmlDocument.parse(svg);
  } on XmlException catch (e) {
    return _notXml(e.message);
  }
  if (doc.children.any((n) => n is XmlDoctype)) {
    return (code: 'doctype', message: 'icon.svg has a <!DOCTYPE>; leave it out.');
  }
  if (_strictProblem(svg, doc) case final why?) return _notXml(why);
  final root = doc.rootElement;
  if (root.name.local != 'svg') {
    return (
      code: 'not_svg',
      message:
          'icon.svg must have <svg> as its root element, not '
          '<${root.name.local}>.',
    );
  }
  if (_squareViewBox(root) case final p?) return p;
  for (final e in root.descendantElements.followedBy([root])) {
    final name = e.name.local;
    if (!iconElements.contains(name)) {
      return (
        code: 'element',
        message:
            'icon.svg uses <$name>, which an icon may not: use only '
            '${iconElements.join(', ')}. No scripts, foreignObject, images, '
            'filters, styles or animation.',
      );
    }
    for (final a in e.attributes) {
      if (_attributeProblem(a) case final p?) return p;
    }
  }
  return _useProblem(root);
}

IconProblem _notXml(String why) =>
    (code: 'not_xml', message: 'icon.svg is not well-formed XML: $why.');

/// Refuses `<use>`s that expand past [maxIconUses] copies or nest deeper
/// than [maxIconUseDepth] (a reference cycle among them).
IconProblem? _useProblem(XmlElement root) {
  final byId = <String, XmlElement>{
    for (final e in root.descendantElements) ?e.getAttribute('id'): e,
  };
  var copies = 0;
  // Counts the copies [e]'s subtree draws; the code of the limit it
  // passes, or null.
  String? walk(XmlElement e, int depth) {
    for (final u in [e, ...e.descendantElements]) {
      if (u.name.local != 'use') continue;
      if (depth >= maxIconUseDepth) return 'use_too_deep';
      if (++copies > maxIconUses) return 'use_too_many';
      final href = u.getAttribute('href') ?? u.getAttribute('xlink:href');
      final target = href == null || href.isEmpty
          ? null
          : byId[href.substring(1)];
      if (target != null) {
        if (walk(target, depth + 1) case final code?) return code;
      }
    }
    return null;
  }

  final code = walk(root, 0);
  if (code == null) return null;
  return (
    code: code,
    message:
        'icon.svg\'s <use> elements expand to more than $maxIconUses '
        'copies, or nest more than $maxIconUseDepth deep: draw the shapes '
        'directly instead.',
  );
}

/// [svg] with every element outside [iconElements] and every unsafe
/// attribute dropped, or null when it is too large, not well-formed, not
/// an `<svg>`, nested too deep, not square or past the `<use>` limits. It reads XML as
/// `XmlDocument.parse` does, without [iconProblem]'s stricter checks: an
/// older icon the core would now refuse for its XML alone is still drawn.
String? cleanIcon(String svg) {
  if (utf8.encode(svg).length > maxIconBytes || _tooDeep(svg)) return null;
  final XmlDocument doc;
  try {
    doc = XmlDocument.parse(svg);
  } on XmlException {
    return null;
  }
  if (doc.children.any((n) => n is XmlDoctype)) return null;
  final root = doc.rootElement;
  if (root.name.local != 'svg' || _squareViewBox(root) != null) return null;
  if (_useProblem(root) != null) return null;
  void clean(XmlElement e) {
    e.attributes.removeWhere((a) => _attributeProblem(a) != null);
    e.children.removeWhere(
      (n) =>
          (n is XmlElement && !iconElements.contains(n.name.local)) ||
          n is XmlProcessing,
    );
    for (final c in e.childElements) {
      clean(c);
    }
  }

  clean(root);
  return root.toXmlString();
}

/// Whether [s]'s start tags nest more than [maxIconDepth] deep, by a scan
/// of the text that needs no parse, as the core's: a start tag not closed
/// by `/>` opens a level and an end tag closes one; a comment,
/// `<![CDATA[`, `<?…?>` and `<!…>` open none, and a quoted value in a tag
/// hides its `>`.
bool _tooDeep(String s) {
  // Where [end] ends, searching from [from]; the text's end without one.
  int past(String end, int from) {
    if (from > s.length) return s.length;
    final at = s.indexOf(end, from);
    return at < 0 ? s.length : at + end.length;
  }

  var depth = 0;
  var i = 0;
  while (i < s.length) {
    if (s.startsWith('<!--', i)) {
      i = past('-->', i + 4);
    } else if (s.startsWith('<![CDATA[', i)) {
      i = past(']]>', i + 9);
    } else if (s.startsWith('<?', i)) {
      i = past('?>', i + 2);
    } else if (s.startsWith('<!', i)) {
      i = past('>', i + 2);
    } else if (s.startsWith('</', i)) {
      if (depth > 0) depth--;
      i = past('>', i + 2);
    } else if (s[i] == '<') {
      var j = i + 1;
      while (j < s.length && s[j] != '>') {
        if (s[j] == '"' || s[j] == "'") j = past(s[j], j + 1) - 1;
        j++;
      }
      if (j >= s.length || s[j - 1] != '/') {
        if (++depth > maxIconDepth) return true;
      }
      i = j + 1;
    } else {
      final at = s.indexOf('<', i);
      i = at < 0 ? s.length : at;
    }
  }
  return false;
}

/// A number as SVG writes one; the core reads a viewBox's alike.
final _svgNumber = RegExp(r'^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$');

IconProblem? _squareViewBox(XmlElement root) {
  final box = root.getAttribute('viewBox');
  if (box == null) {
    return (
      code: 'view_box',
      message:
          'icon.svg needs a square viewBox, such as viewBox="0 0 1024 1024".',
    );
  }
  final parts = box
      .trim()
      .split(RegExp(r'[\s,]+'))
      .map((p) => _svgNumber.hasMatch(p) ? double.tryParse(p) : null)
      .toList();
  if (parts.length != 4 || parts.any((p) => p == null)) {
    return (
      code: 'view_box',
      message: 'icon.svg\'s viewBox "$box" is not four numbers.',
    );
  }
  final (w, h) = (parts[2]!, parts[3]!);
  if (w <= 0 || h <= 0 || (w - h).abs() > 1e-6 * w) {
    return (
      code: 'view_box',
      message:
          'icon.svg\'s viewBox "$box" is not square: give it equal width '
          'and height, such as "0 0 1024 1024".',
    );
  }
  return null;
}

final _localUrl = RegExp(r'^url\(\s*#[^)]*\)$');

IconProblem? _attributeProblem(XmlAttribute a) {
  final name = a.name.local;
  final value = a.value.trim();
  if (name.toLowerCase().startsWith('on')) {
    return (
      code: 'event_attribute',
      message: 'icon.svg: the event attribute $name is not allowed.',
    );
  }
  if (name == 'href') {
    return value.startsWith('#')
        ? null
        : (
            code: 'external_href',
            message:
                'icon.svg: href "$value" points outside the file; only '
                '#references within it are allowed.',
          );
  }
  if (name == 'filter') {
    return (code: 'filter', message: 'icon.svg: filters are not allowed.');
  }
  final lower = value.toLowerCase();
  if (lower.contains('url(')) {
    // Every url() must name an element of the file, as fill="url(#g)".
    for (final m in RegExp(r'url\([^)]*\)').allMatches(lower)) {
      if (!_localUrl.hasMatch(m[0]!)) {
        return (
          code: 'external_url',
          message:
              'icon.svg: $name="$value" points outside the file; only '
              'url(#…) is allowed.',
        );
      }
    }
  }
  if (name == 'style' &&
      (lower.contains('animation') ||
          lower.contains('@') ||
          lower.contains('expression') ||
          lower.contains('filter'))) {
    return (
      code: 'style',
      message:
          'icon.svg: the style "$value" uses animation, filters or '
          'at-rules, which an icon may not.',
    );
  }
  return null;
}

/// What the core's XML parser (roxmltree) refuses that
/// `XmlDocument.parse` reads, in a [doc] parsed from [svg] without a
/// DOCTYPE: a character outside XML's, a `&` that starts no predefined or
/// character reference, an attribute without a quoted value or with a `<`
/// in it, text outside the root element, `]]>` in text, `--` in a comment,
/// an XML declaration after the start, an undeclared namespace prefix, or
/// an attribute written twice.
String? _strictProblem(String svg, XmlDocument doc) {
  for (final r in svg.runes) {
    if (!_isXmlChar(r)) return '${_codePoint(r)} is not an XML character';
  }
  return _markupProblem(svg) ?? _namespaceProblem(doc.rootElement);
}

bool _isXmlChar(int r) =>
    r == 0x9 ||
    r == 0xA ||
    r == 0xD ||
    (r >= 0x20 && r <= 0xD7FF) ||
    (r >= 0xE000 && r <= 0xFFFD) ||
    (r >= 0x10000 && r <= 0x10FFFF);

String _codePoint(int r) =>
    'U+${r.toRadixString(16).toUpperCase().padLeft(4, '0')}';

bool _isXmlSpace(String c) => c == ' ' || c == '\t' || c == '\n' || c == '\r';

final _reference = RegExp(
  r'&(#x[0-9a-fA-F]+|#[0-9]+|[A-Za-z_:][-A-Za-z0-9._:]*);',
);

const _predefined = {'lt', 'gt', 'amp', 'apos', 'quot'};

/// The first `&` in [text] that starts no predefined or character
/// reference to an XML character.
String? _referenceProblem(String text) {
  for (var k = text.indexOf('&'); k >= 0; k = text.indexOf('&', k + 1)) {
    final m = _reference.matchAsPrefix(text, k);
    if (m == null) return 'a & starts no reference (write &amp;)';
    final ref = m[1]!;
    if (ref.startsWith('#')) {
      final code = ref.startsWith('#x')
          ? int.tryParse(ref.substring(2), radix: 16)
          : int.tryParse(ref.substring(1));
      if (code == null || !_isXmlChar(code)) {
        return 'the reference &$ref; is not an XML character';
      }
    } else if (!_predefined.contains(ref)) {
      return 'the entity &$ref; is not one of XML\'s five (write the '
          'character itself)';
    }
  }
  return null;
}

/// Walks [s]'s markup, which `XmlDocument.parse` has read, for the
/// stricter rules that need the text as written.
String? _markupProblem(String s) {
  final start = s.startsWith('﻿') ? 1 : 0;
  var i = start;
  var depth = 0;
  int past(String end, int from) {
    final at = s.indexOf(end, from);
    return at < 0 ? s.length : at + end.length;
  }

  while (i < s.length) {
    if (s.startsWith('<!--', i)) {
      final end = past('-->', i + 4);
      final body = s.substring(i + 4, end - 3);
      if (body.contains('--') || body.endsWith('-')) {
        return 'a comment holds "--"';
      }
      i = end;
    } else if (s.startsWith('<![CDATA[', i)) {
      i = past(']]>', i + 9);
    } else if (s.startsWith('<?', i)) {
      var t = i + 2;
      while (t < s.length && !_isXmlSpace(s[t]) && s[t] != '?') {
        t++;
      }
      final target = s.substring(i + 2, t);
      if (target.toLowerCase() == 'xml' && (target != 'xml' || i != start)) {
        return 'an XML declaration may only open the file';
      }
      i = past('?>', t);
    } else if (s.startsWith('</', i)) {
      depth--;
      i = past('>', i);
    } else if (s[i] == '<') {
      final (end, problem, open) = _startTag(s, i);
      if (problem != null) return problem;
      if (open) depth++;
      i = end;
    } else {
      final at = s.indexOf('<', i);
      final end = at < 0 ? s.length : at;
      final text = s.substring(i, end);
      if (depth == 0 && text.split('').any((c) => !_isXmlSpace(c))) {
        return 'there is text outside the root element';
      }
      if (text.contains(']]>')) return 'text holds "]]>"';
      if (_referenceProblem(text) case final p?) return p;
      i = end;
    }
  }
  return null;
}

/// The start tag at [at] in [s]: where it ends, its first stricter
/// problem, and whether it opens an element (is not self-closing).
(int, String?, bool) _startTag(String s, int at) {
  var i = at + 1;
  bool more() => i < s.length;
  while (more() && !_isXmlSpace(s[i]) && s[i] != '/' && s[i] != '>') {
    i++;
  }
  while (true) {
    while (more() && _isXmlSpace(s[i])) {
      i++;
    }
    if (!more()) return (i, null, false);
    if (s[i] == '>') return (i + 1, null, true);
    if (s.startsWith('/>', i)) return (i + 2, null, false);
    final from = i;
    while (more() &&
        !_isXmlSpace(s[i]) &&
        s[i] != '=' &&
        s[i] != '>' &&
        !s.startsWith('/>', i)) {
      i++;
    }
    final name = s.substring(from, i);
    while (more() && _isXmlSpace(s[i])) {
      i++;
    }
    if (!more() || s[i] != '=') {
      return (i, 'the attribute $name has no value', false);
    }
    i++;
    while (more() && _isXmlSpace(s[i])) {
      i++;
    }
    if (!more() || (s[i] != '"' && s[i] != "'")) {
      return (i, 'the value of $name is not quoted', false);
    }
    final quote = s[i];
    final close = s.indexOf(quote, i + 1);
    final value = s.substring(i + 1, close < 0 ? s.length : close);
    if (value.contains('<')) return (i, 'the value of $name holds a <', false);
    if (_referenceProblem(value) case final p?) return (i, p, false);
    i = close < 0 ? s.length : close + 1;
  }
}

/// The first prefix used on [root] or below that no `xmlns:` declares, or
/// an attribute written twice, in document order.
String? _namespaceProblem(XmlElement root) {
  final stack = <(XmlElement, Set<String>)>[(root, const {})];
  while (stack.isNotEmpty) {
    final (e, scope) = stack.removeLast();
    final declared = {
      ...scope,
      for (final a in e.attributes)
        if (a.name.prefix == 'xmlns') a.name.local,
    };
    final seen = <String>{};
    for (final a in e.attributes) {
      final qualified = a.name.qualified;
      if (!seen.add(qualified)) {
        return 'the attribute $qualified is written twice on '
            '<${e.name.qualified}>';
      }
      final prefix = a.name.prefix;
      if (prefix != null &&
          prefix != 'xml' &&
          prefix != 'xmlns' &&
          !declared.contains(prefix)) {
        return 'the prefix of $qualified is not declared (add '
            'xmlns:$prefix="…" to <svg>)';
      }
    }
    final prefix = e.name.prefix;
    if (prefix != null && prefix != 'xml' && !declared.contains(prefix)) {
      return 'the prefix of <${e.name.qualified}> is not declared';
    }
    for (final c in e.childElements.toList().reversed) {
      stack.add((c, declared));
    }
  }
  return null;
}
