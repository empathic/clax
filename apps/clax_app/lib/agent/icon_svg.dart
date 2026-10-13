/// An app's launcher icon: the `icon.svg` among its files, a square
/// full-bleed design in a small, safe subset of SVG.
///
/// [iconProblem] is the agent's check when it writes the file; [cleanIcon]
/// is the reader's, which drops what the subset leaves out (an older app or
/// a hand edit may carry it) and refuses what cannot be drawn.
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

/// Why [svg] is not a usable icon, as a sentence for the model to act on;
/// null when it is one.
String? iconProblem(String svg) {
  final bytes = utf8.encode(svg).length;
  if (bytes > maxIconBytes) {
    return 'icon.svg is $bytes bytes; an icon may be at most 32 KB '
        '($maxIconBytes bytes): simplify its paths.';
  }
  final XmlDocument doc;
  try {
    doc = XmlDocument.parse(svg);
  } on XmlException catch (e) {
    return 'icon.svg is not well-formed XML: ${e.message}.';
  }
  if (doc.children.any((n) => n is XmlDoctype)) {
    return 'icon.svg has a <!DOCTYPE>; leave it out.';
  }
  final root = doc.rootElement;
  if (root.name.local != 'svg') {
    return 'icon.svg must have <svg> as its root element, not '
        '<${root.name.local}>.';
  }
  if (_squareViewBox(root) case final p?) return p;
  for (final e in root.descendantElements.followedBy([root])) {
    final name = e.name.local;
    if (!iconElements.contains(name)) {
      return 'icon.svg uses <$name>, which an icon may not: use only '
          '${iconElements.join(', ')}. No scripts, foreignObject, images, '
          'filters, styles or animation.';
    }
    for (final a in e.attributes) {
      if (_attributeProblem(a) case final p?) return 'icon.svg: $p';
    }
  }
  return null;
}

/// [svg] with every element outside [iconElements] and every unsafe
/// attribute dropped, or null when it is too large, not well-formed, not
/// an `<svg>` or not square.
String? cleanIcon(String svg) {
  if (utf8.encode(svg).length > maxIconBytes) return null;
  final XmlDocument doc;
  try {
    doc = XmlDocument.parse(svg);
  } on XmlException {
    return null;
  }
  if (doc.children.any((n) => n is XmlDoctype)) return null;
  final root = doc.rootElement;
  if (root.name.local != 'svg' || _squareViewBox(root) != null) return null;
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

String? _squareViewBox(XmlElement root) {
  final box = root.getAttribute('viewBox');
  if (box == null) {
    return 'icon.svg needs a square viewBox, such as viewBox="0 0 1024 1024".';
  }
  final parts = box
      .trim()
      .split(RegExp(r'[\s,]+'))
      .map(double.tryParse)
      .toList();
  if (parts.length != 4 || parts.any((p) => p == null)) {
    return 'icon.svg\'s viewBox "$box" is not four numbers.';
  }
  final (w, h) = (parts[2]!, parts[3]!);
  if (w <= 0 || h <= 0 || (w - h).abs() > 1e-6 * w) {
    return 'icon.svg\'s viewBox "$box" is not square: give it equal width '
        'and height, such as "0 0 1024 1024".';
  }
  return null;
}

final _localUrl = RegExp(r'^url\(\s*#[^)]*\)$');

String? _attributeProblem(XmlAttribute a) {
  final name = a.name.local;
  final value = a.value.trim();
  if (name.toLowerCase().startsWith('on')) {
    return 'the event attribute $name is not allowed.';
  }
  if (name == 'href') {
    return value.startsWith('#')
        ? null
        : 'href "$value" points outside the file; only #references within '
              'it are allowed.';
  }
  if (name == 'filter') return 'filters are not allowed.';
  final lower = value.toLowerCase();
  if (lower.contains('url(')) {
    // Every url() must name an element of the file, as fill="url(#g)".
    for (final m in RegExp(r'url\([^)]*\)').allMatches(lower)) {
      if (!_localUrl.hasMatch(m[0]!)) {
        return '$name="$value" points outside the file; only url(#…) is '
            'allowed.';
      }
    }
  }
  if (name == 'style' &&
      (lower.contains('animation') ||
          lower.contains('@') ||
          lower.contains('expression') ||
          lower.contains('filter'))) {
    return 'the style "$value" uses animation, filters or at-rules, which an '
        'icon may not.';
  }
  return null;
}
