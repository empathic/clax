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
/// parser), no DOCTYPE (read from the text too), the XML declaration and
/// processing instructions as the core's parser reads them
/// ([_instructions]), well-formed XML, the core parser's stricter XML
/// ([_strictProblem]), an `<svg>` root, a square `viewBox`, then each
/// element (the root's descendants in document order, the root last) by its
/// name and its attributes, then the `<use>` expansion.
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
  if (_hasDoctype(svg)) {
    return (code: 'doctype', message: 'icon.svg has a <!DOCTYPE>; leave it out.');
  }
  final (piProblem, withoutPis) = _instructions(svg);
  if (piProblem != null) return _notXml(piProblem);
  final XmlDocument doc;
  try {
    doc = XmlDocument.parse(withoutPis);
  } on XmlException catch (e) {
    return _notXml(e.message);
  }
  // A doctype written other than `<!DOCTYPE`, which the core's parser
  // does not read as one.
  if (doc.children.any((n) => n is XmlDoctype)) {
    return _notXml('a document type must be written <!DOCTYPE');
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
/// attribute (and every processing instruction) dropped, or null when it is
/// too large, nested too deep, not well-formed, not an `<svg>`, not square
/// or past the `<use>` limits. It reads XML as `XmlDocument.parse` does,
/// without [iconProblem]'s stricter checks: an older icon the core would
/// now refuse for its XML alone is still drawn.
String? cleanIcon(String svg) {
  if (utf8.encode(svg).length > maxIconBytes ||
      _tooDeep(svg) ||
      _hasDoctype(svg)) {
    return null;
  }
  final (piProblem, withoutPis) = _instructions(svg);
  final XmlDocument doc;
  try {
    doc = XmlDocument.parse(piProblem == null ? withoutPis : svg);
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

/// Whether [s] holds a `<!DOCTYPE` outside comments, CDATA sections,
/// processing instructions and quoted values, by a scan of the text that
/// needs no parse, as the core's.
bool _hasDoctype(String s) {
  int past(String end, int from) {
    if (from > s.length) return s.length;
    final at = s.indexOf(end, from);
    return at < 0 ? s.length : at + end.length;
  }

  var i = 0;
  while (i < s.length) {
    if (s.startsWith('<!DOCTYPE', i)) {
      return true;
    } else if (s.startsWith('<!--', i)) {
      i = past('-->', i + 4);
    } else if (s.startsWith('<![CDATA[', i)) {
      i = past(']]>', i + 9);
    } else if (s.startsWith('<?', i)) {
      i = past('?>', i + 2);
    } else if (s[i] == '<') {
      var j = i + 1;
      while (j < s.length && s[j] != '>') {
        if (s[j] == '"' || s[j] == "'") j = past(s[j], j + 1) - 1;
        j++;
      }
      i = j + 1;
    } else {
      final at = s.indexOf('<', i);
      i = at < 0 ? s.length : at;
    }
  }
  return false;
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
/// in it, text or a CDATA section outside the root element, `]]>` in text,
/// `--` in a comment, a name with more than one `:` or an empty part, an
/// undeclared namespace prefix, a namespace bound against XML's rules, or
/// an attribute written twice (under one name, or two prefixes bound to
/// one namespace).
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
      if (depth == 0) return 'there is a CDATA section outside the root element';
      i = past(']]>', i + 9);
    } else if (s.startsWith('<?', i)) {
      // Checked by _instructions.
      i = past('?>', i + 2);
    } else if (s.startsWith('</', i)) {
      final n = _qnameLength(s, i + 2);
      var j = i + 2 + n;
      while (j < s.length && _isXmlSpace(s[j])) {
        j++;
      }
      if (n == 0 || j >= s.length || s[j] != '>') {
        return 'an end tag is not </name>';
      }
      depth--;
      i = j + 1;
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
/// problem, and whether it opens an element (is not self-closing). Its
/// name and each attribute's are qualified XML names, and whitespace comes
/// before each attribute.
(int, String?, bool) _startTag(String s, int at) {
  final n = _qnameLength(s, at + 1);
  if (n == 0) return (at, 'a start tag names no element', false);
  var i = at + 1 + n;
  bool more() => i < s.length;
  while (true) {
    final from = i;
    while (more() && _isXmlSpace(s[i])) {
      i++;
    }
    if (!more()) return (i, null, false);
    if (s[i] == '>') return (i + 1, null, true);
    if (s.startsWith('/>', i)) return (i + 2, null, false);
    if (i == from) return (i, 'an attribute follows without whitespace', false);
    final length = _qnameLength(s, i);
    if (length == 0) return (i, 'an attribute\'s name is not an XML name', false);
    final name = s.substring(i, i + length);
    i += length;
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

const _xmlUri = 'http://www.w3.org/XML/1998/namespace';
const _xmlnsUri = 'http://www.w3.org/2000/xmlns/';

/// The first namespace problem on [root] or below, in document order, as
/// the core's parser finds them: a name with more than one `:` or an empty
/// part; `xmlns:xml` bound to another URI, another prefix (or the default
/// namespace) bound to XML's or to `xmlns`'s URI; an element prefixed
/// `xmlns`, or an undeclared prefix (`xml` is declared for attributes
/// only); or an attribute written twice, under one name or under two
/// prefixes bound to one URI. An attribute named `xmlns` under any prefix
/// declares the default namespace, as the core's parser reads it.
String? _namespaceProblem(XmlElement root) {
  final stack = <(XmlElement, Map<String, String>)>[(root, const {})];
  while (stack.isNotEmpty) {
    final (e, scope) = stack.removeLast();
    final element = e.name.qualified;
    if (_qnameProblem(element) case final p?) return p;
    final declared = {...scope};
    final seen = <String>{};
    for (final a in e.attributes) {
      final qualified = a.name.qualified;
      if (_qnameProblem(qualified) case final p?) return p;
      final (prefix, local) = _split(qualified);
      // The default namespace may be declared more than once.
      if ((prefix == 'xmlns' || local != 'xmlns') && !seen.add(qualified)) {
        return 'the attribute $qualified is written twice on <$element>';
      }
      if (prefix == 'xmlns') {
        if (a.value == _xmlnsUri) return 'xmlns:$local binds xmlns\'s URI';
        if ((local == 'xml') != (a.value == _xmlUri)) {
          return 'xmlns:$local binds the xml prefix or URI to another';
        }
        if (local != 'xml') declared[local] = a.value;
      } else if (local == 'xmlns') {
        if (a.value == _xmlUri || a.value == _xmlnsUri) {
          return 'the default namespace is bound to a reserved URI';
        }
      }
    }
    // Attributes by namespace URI and local name.
    final names = <String>{};
    for (final a in e.attributes) {
      final (prefix, local) = _split(a.name.qualified);
      if (prefix == 'xmlns' || local == 'xmlns') continue;
      final String uri;
      if (prefix == null) {
        uri = '';
      } else if (prefix == 'xml') {
        uri = _xmlUri;
      } else if (declared[prefix] case final u?) {
        uri = '\u0000$u';
      } else {
        return 'the prefix of ${a.name.qualified} is not declared (add '
            'xmlns:$prefix="…" to <svg>)';
      }
      if (!names.add('$uri $local')) {
        return 'the attribute $local is written twice on <$element> (its '
            'prefixes name one namespace)';
      }
    }
    final (prefix, _) = _split(element);
    if (prefix != null && !declared.containsKey(prefix)) {
      return 'the prefix of <$element> is not declared';
    }
    if (prefix == 'xmlns') return '<$element> takes the xmlns prefix';
    for (final c in e.childElements.toList().reversed) {
      stack.add((c, declared));
    }
  }
  return null;
}

/// A qualified name's prefix (null without one, or with an empty one) and
/// local name.
(String?, String) _split(String qualified) {
  final at = qualified.indexOf(':');
  if (at <= 0) return (null, qualified.substring(at + 1));
  return (qualified.substring(0, at), qualified.substring(at + 1));
}

/// Why [qualified] is not a name the core's parser reads ([_qnameLength]):
/// a character outside XML's names, more than one `:`, or an empty part.
String? _qnameProblem(String qualified) {
  if (_qnameLength(qualified, 0) != qualified.length) {
    return 'the name $qualified is not a qualified XML name';
  }
  return null;
}

/// [s]'s XML declaration and processing instructions as the core's parser
/// reads them, which `XmlDocument.parse` reads otherwise: the first problem
/// and [s] without them (they draw nothing), for `XmlDocument.parse` to
/// read. A declaration is `<?xml ` (a space after it) only at the start,
/// after one byte order mark, with `version`, then optionally `encoding`
/// and `standalone`, in that order; anywhere else `<?xml ` is a problem.
/// Any other `<?` names a target, an XML name (`<?xml?>` and `<?XML x?>`
/// are instructions), and ends at `?>`.
(String?, String) _instructions(String s) {
  final start = s.startsWith('﻿') ? 1 : 0;
  final out = StringBuffer();
  var copied = 0;
  int past(String end, int from) {
    if (from > s.length) return s.length;
    final at = s.indexOf(end, from);
    return at < 0 ? s.length : at + end.length;
  }

  var i = start;
  while (i < s.length) {
    if (s.startsWith('<!--', i)) {
      i = past('-->', i + 4);
    } else if (s.startsWith('<![CDATA[', i)) {
      i = past(']]>', i + 9);
    } else if (s.startsWith('<!', i)) {
      i = _pastMarkupDeclaration(s, i);
    } else if (s.startsWith('<?', i)) {
      final int end;
      if (s.startsWith('<?xml ', i)) {
        if (i != start) {
          return ('an XML declaration may only open the file', s);
        }
        final (problem, after) = _declaration(s, i);
        if (problem != null) return (problem, s);
        end = after;
      } else {
        if (_nameLength(s, i + 2) == 0) {
          return ('a processing instruction names no target', s);
        }
        final at = s.indexOf('?>', i + 2);
        if (at < 0) return ('a processing instruction is not closed', s);
        end = at + 2;
      }
      out.write(s.substring(copied, i));
      copied = end;
      i = end;
    } else if (s[i] == '<') {
      var j = i + 1;
      while (j < s.length && s[j] != '>') {
        if (s[j] == '"' || s[j] == "'") j = past(s[j], j + 1) - 1;
        j++;
      }
      i = j + 1;
    } else {
      final at = s.indexOf('<', i);
      i = at < 0 ? s.length : at;
    }
  }
  out.write(s.substring(copied.clamp(0, s.length)));
  return (null, out.toString());
}

/// Where the `<!…>` at [at] ends: past its `>`, skipping quoted strings
/// and a `[…]` internal subset.
int _pastMarkupDeclaration(String s, int at) {
  var j = at + 2;
  while (j < s.length && s[j] != '>') {
    if (s[j] == '"' || s[j] == "'") {
      final close = s.indexOf(s[j], j + 1);
      j = close < 0 ? s.length : close + 1;
    } else if (s[j] == '[') {
      j++;
      while (j < s.length && s[j] != ']') {
        if (s[j] == '"' || s[j] == "'") {
          final close = s.indexOf(s[j], j + 1);
          j = close < 0 ? s.length : close;
        }
        j++;
      }
      j++;
    } else {
      j++;
    }
  }
  return j >= s.length ? s.length : j + 1;
}

/// The XML declaration at [at]: its problem, and where it ends. It is
/// `<?xml`, whitespace, `version="…"`, then `encoding="…"` and
/// `standalone="…"` if present, each after whitespace, and `?>` (the
/// values are not read).
(String?, int) _declaration(String s, int at) {
  const bad = 'the XML declaration is not version, encoding, standalone';
  var i = at + 5;
  bool space(int k) => k < s.length && _isXmlSpace(s[k]);
  // Whitespace, or else `?>` or the end must follow.
  bool spaces() {
    if (space(i)) {
      while (space(i)) {
        i++;
      }
      return true;
    }
    return s.startsWith('?>', i) || i >= s.length;
  }

  // name, `=` with optional whitespace around it, a quoted value without <.
  bool attribute() {
    final n = _qnameLength(s, i);
    if (n == 0) return false;
    i += n;
    while (space(i)) {
      i++;
    }
    if (i >= s.length || s[i] != '=') return false;
    i++;
    while (space(i)) {
      i++;
    }
    if (i >= s.length || (s[i] != '"' && s[i] != "'")) return false;
    final quote = s[i];
    i++;
    while (i < s.length && s[i] != quote && s[i] != '<') {
      i++;
    }
    if (i >= s.length || s[i] != quote) return false;
    i++;
    return true;
  }

  if (!spaces() || !s.startsWith('version', i) || !attribute()) {
    return (bad, i);
  }
  if (!spaces()) return (bad, i);
  if (s.startsWith('encoding', i)) {
    if (!attribute() || !spaces()) return (bad, i);
  }
  if (s.startsWith('standalone', i) && !attribute()) return (bad, i);
  while (space(i)) {
    i++;
  }
  if (!s.startsWith('?>', i)) return (bad, i);
  return (null, i + 2);
}

/// The UTF-16 length of the XML name at [at] (a name start character, then
/// name characters), 0 when none starts there.
int _nameLength(String s, int at) {
  var i = at;
  while (i < s.length) {
    final r = _runeAt(s, i);
    if (!(i == at ? _isNameStart(r) : _isNameChar(r))) break;
    i += r > 0xFFFF ? 2 : 1;
  }
  return i - at;
}

/// The UTF-16 length of the qualified name at [at] as the core's parser
/// reads one: name characters with at most one `:`, the prefix and the
/// local name each starting with a name start character; 0 when none.
int _qnameLength(String s, int at) {
  var i = at;
  int? colon;
  while (i < s.length) {
    final r = _runeAt(s, i);
    if (r == 0x3A) {
      if (colon != null) return 0;
      colon = i;
    } else if (!_isNameChar(r)) {
      break;
    }
    i += r > 0xFFFF ? 2 : 1;
  }
  final local = colon == null ? at : colon + 1;
  if (local >= i || !_isNameStart(_runeAt(s, local))) return 0;
  if (colon != null && colon > at && !_isNameStart(_runeAt(s, at))) return 0;
  return i - at;
}

int _runeAt(String s, int i) {
  final c = s.codeUnitAt(i);
  if (c >= 0xD800 && c <= 0xDBFF && i + 1 < s.length) {
    final d = s.codeUnitAt(i + 1);
    if (d >= 0xDC00 && d <= 0xDFFF) {
      return 0x10000 + ((c - 0xD800) << 10) + (d - 0xDC00);
    }
  }
  return c;
}

/// XML's NameStartChar, as the core's parser reads it.
bool _isNameStart(int r) {
  if (r <= 128) {
    return (r >= 0x41 && r <= 0x5A) ||
        (r >= 0x61 && r <= 0x7A) ||
        r == 0x3A ||
        r == 0x5F;
  }
  return (r >= 0xC0 && r <= 0xD6) ||
      (r >= 0xD8 && r <= 0xF6) ||
      (r >= 0xF8 && r <= 0x2FF) ||
      (r >= 0x370 && r <= 0x37D) ||
      (r >= 0x37F && r <= 0x1FFF) ||
      (r >= 0x200C && r <= 0x200D) ||
      (r >= 0x2070 && r <= 0x218F) ||
      (r >= 0x2C00 && r <= 0x2FEF) ||
      (r >= 0x3001 && r <= 0xD7FF) ||
      (r >= 0xF900 && r <= 0xFDCF) ||
      (r >= 0xFDF0 && r <= 0xFFFD) ||
      (r >= 0x10000 && r <= 0xEFFFF);
}

/// XML's NameChar, as the core's parser reads it.
bool _isNameChar(int r) {
  if (r <= 128) {
    return _isNameStart(r) ||
        (r >= 0x30 && r <= 0x39) ||
        r == 0x2D ||
        r == 0x2E;
  }
  return _isNameStart(r) ||
      r == 0xB7 ||
      (r >= 0x300 && r <= 0x36F) ||
      (r >= 0x203F && r <= 0x2040);
}
