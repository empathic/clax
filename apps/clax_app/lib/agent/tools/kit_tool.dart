/// The `add_kit` tool: copies the vendored Clax kit into the draft.
library;

import 'dart:convert';

import 'package:flutter/services.dart' show rootBundle;

import 'file_tools.dart' show maxDraftFiles;
import 'tool_spec.dart';
import 'toolbox.dart';

/// Reads one kit file by its name in `assets/kit/` (such as `kit.js`).
typedef KitLoader = Future<String> Function(String name);

/// Each part the tool takes and the kit files it brings, its own and those
/// of the parts it needs, in the order they are added.
const kitParts = <String, List<String>>{
  'preact': ['preact.js', 'LICENSES.txt'],
  'kit': ['preact.js', 'icons.js', 'kit.css', 'kit.js', 'LICENSES.txt'],
  'chart': ['chart.js'],
  'map': ['preact.js', 'icons.js', 'map.js', 'LICENSES.txt'],
  'dates': ['dates.js', 'LICENSES.txt'],
  'icons': ['icons.js', 'LICENSES.txt'],
};

/// The directory of the draft the kit files go in.
const kitDir = 'kit';

/// Reads the kit from the app's bundled assets.
Future<String> bundledKit(String name) =>
    rootBundle.loadString('assets/kit/$name');

/// Copies the parts of the kit a page imports into the draft under
/// `kit/`. A file already there with the same text is left alone, so a
/// repeated call changes nothing.
class AddKitTool implements Tool {
  AddKitTool({KitLoader? load}) : _load = load ?? bundledKit;

  final KitLoader _load;

  @override
  final spec = ToolSpec(
    'add_kit',
    'Adds the Clax kit to the draft under kit/: preact (Preact, hooks and '
        'htm as kit/preact.js), kit (kit.css, the design system, and kit.js, '
        'its components; brings preact and icons), chart (SVG charts), '
        'dates (dayjs with relativeTime), icons (about 100 icons), map (a '
        'slippy map; its tiles need the net capability). The '
        'result lists the API. Pages link ./kit/kit.css and import '
        "'./kit/<part>.js'. Repeating it is harmless.",
    objectSchema({
      'parts': {
        'type': 'array',
        'items': {'type': 'string', 'enum': kitParts.keys.toList()},
      },
    }),
  );

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    final parts = switch (input['parts']) {
      final List<Object?> l when l.every(kitParts.containsKey) =>
        l.cast<String>(),
      _ => throw ToolInputError(
        'parts must be a list of ${kitParts.keys.join(', ')}.',
      ),
    };
    if (parts.isEmpty) {
      throw ToolInputError(
        'parts is empty; name at least one of ${kitParts.keys.join(', ')}.',
      );
    }
    final names = <String>{for (final p in parts) ...kitParts[p]!};
    final d = s.draft;
    final added = names.where((n) => !d.files.containsKey('$kitDir/$n'));
    if (d.files.length + added.length > maxDraftFiles) {
      throw ToolInputError(
        'The draft would have ${d.files.length + added.length} files; it may '
        'have at most $maxDraftFiles. Delete some first.',
      );
    }
    final texts = {for (final n in names) n: await _load(n)};
    final out = <Map<String, Object?>>[];
    for (final n in names) {
      final path = '$kitDir/$n';
      final text = texts[n]!;
      final same = d.files[path] == text;
      if (!same) d.writeFile(path, text);
      out.add({
        'path': path,
        'bytes': utf8.encode(text).length,
        'written': !same,
      });
    }
    return ToolOk({
      'files': out,
      'api': [for (final n in names) ?kitApi[n]].join('\n'),
    });
  }
}

/// What each file offers, for the model: the names, the props and the
/// shape of a call. The one source of the kit's API: `add_kit`'s result
/// and the tool loop's system prompt ([kitApiSection]) both read it.
const kitApi = <String, String>{
  'preact.js':
      r"kit/preact.js exports h, html, render, Fragment, createContext, "
      r'useState, useEffect, useMemo, useRef, useReducer, useCallback, '
      r'useContext, useLayoutEffect. Write views as html`<${Comp} prop=${v}/>` '
      r"(close a component with <//>), render(html`<${App} …/>`, "
      r'document.body), from <script type="module">.',
  'kit.css':
      r'kit/kit.css: tokens --k-accent, --k-accent-2 (gradient end), '
      r'--k-bg, --k-surface, --k-fg, --k-fg-2, --k-line, --k-radius, '
      r'--k-chart-1..6; give the app its own look by overriding --k-accent '
      r'and --k-accent-2 on :root and in both dark blocks. Classes: '
      r'k-section (gutter block), k-stack, k-row, k-spread, k-grid-2, '
      r'k-grid-3, k-large, k-title, k-title-2, k-title-3, k-headline, '
      r'k-subhead, k-footnote, k-caption, k-muted, k-num.',
  'kit.js':
      r'kit/kit.js exports: App {routes: {"/": Home, "/item/:id": Item, '
      r'"*": NotFound}, tabs: [{path, label, icon, badge}], theme} (screens '
      r'get {params, path, query}; App draws the TabBar from tabs); go(path), '
      r'back(), useRoute(); AppBar {title, subtitle, large, back: true|path|'
      r'function, backLabel, leading, actions}; TabBar {tabs}; Section '
      r'{title, action}; List {header, footer}; ListItem {title, subtitle, '
      r'trail, lead: icon, leadColor, avatar: name, check: {checked, '
      r'onChange}, onClick, href, destructive, done}; Check {checked, '
      r'onChange, label}; Card {title, subtitle, action, hero, onClick}; '
      r'Button {variant: filled|tonal|outline|plain|danger|gradient, size: '
      r'small|large, block, icon, loading}; IconButton {icon, label, '
      r'onClick}; Field {label, type: text|number|date|time|email|textarea|'
      r'select|toggle, value, onChange(value), options, placeholder, hint, '
      r'error}; SegmentedControl {options, value, onChange}; Chips {options: '
      r'[str|{value, label, icon}], value, onChange, multiple, wrap}; Stat '
      r'{label, value, unit, delta, deltaLabel, icon, invert}; Empty {icon, '
      r'title, body, action}; Fab {icon, label, onClick}; Sheet {open, '
      r'onClose, title, action: {label, onClick, disabled}}; Dialog {open, '
      r'onClose, title, body, actions: [{label, variant, onClick}]}; '
      r'toast(message, {tone: success|danger}) (App hosts the Toast '
      r'component); Badge {tone}; Progress {value, max}; Icon {name, size}; '
      r'Markup {html} (for chart markup); useDoc(path, initial) -> {data, '
      r'save(next|fn), ready, error}; useCollection(path, {orderBy, dir, '
      r'limit}) -> {items: [{id, …}], ready, error, add(data) -> id, set(id, '
      r'data), update(id, patch), remove(id)}. Both use the db capability '
      r'when declared ({"db": {}}), else memory (lost on reload; getDb() '
      r'resolves null then); a failed write shows a toast and rejects. '
      r'uid(); fmt.number, fmt.money(v, currency), fmt.percent.',
  'chart.js':
      r'kit/chart.js exports functions returning SVG markup (show with '
      r'<${Markup} html=${…}/>): line(data, opts), area(data, opts), '
      r'bar(data, {stacked}), pie({items: [{label, value, color}]}, {donut, '
      r'center, centerSub}), donut(…), sparkline(values, {width, height, '
      r'color}); data is {labels, series: [{name, values, color}]} or '
      r'{labels, values}; opts {height, format, legend}. Also compact(n) '
      r'(1200 -> 1.2k) and ticks(min, max, count).',
  'map.js':
      r'kit/map.js exports MapView {center: [lat, lng], zoom, fit: points '
      r'(fits the view to them; points are [lat, lng] or {lat, lng}), '
      r'markers: [{lat, lng, label, color, icon, selected, onClick(marker)}], '
      r"here: {lat, lon, accuracy} (the location capability's current(), a "
      r'"you are here" dot), onMove({center, zoom, bounds: {north, south, '
      r'east, west}}), height, tiles: "https://host/{z}/{x}/{y}.png", '
      r'attribution, maxZoom, darkTiles}; people drag, pinch, scroll, '
      r'double-tap and use the arrow keys. Tiles come through the net '
      r'capability as blob: images, so the app declares {"net": {"hosts": '
      r'["tile.openstreetmap.org"]}} (or the host of its tiles): the owner is asked '
      r'once, and the map says why it is empty when not allowed. In your own '
      r'renders net is off, so the map draws a grey "Map preview" grid '
      r'instead of tiles: that is expected; keep the map and its net '
      r'declaration. The '
      r'OpenStreetMap attribution always shows. Also fitBounds(points, '
      r'{width, height, padding}) -> {center, zoom}, project(lat, lng) -> '
      r'[x, y], unproject(x, y) -> [lat, lng], OSM {tiles, attribution, '
      r'maxZoom}.',
  'dates.js':
      r"kit/dates.js default-exports dayjs with relativeTime: "
      r"dayjs(x).fromNow(), .format('D MMM'), .add(1, 'day'), "
      r".isSame(y, 'day').",
  'icons.js':
      r'kit/icons.js: icon(name, {size, stroke}) -> SVG markup, and names. '
      r'Icon names (kit.js takes them as icon props): home search plus minus '
      r'x check chevronLeft chevronRight chevronDown chevronUp arrowLeft '
      r'arrowRight arrowUp arrowDown menu more moreVertical settings user '
      r'users heart star bookmark calendar clock bell mail phone message send '
      r'share trash pencil edit copy download upload filter sliders list grid '
      r'image camera pin map globe link lock unlock eye eyeOff info alert '
      r'warning help checkCircle xCircle plusCircle refresh play pause stop '
      r'skip music mic volume sun moon cloud zap flame droplet leaf coffee '
      r'utensils cart bag tag wallet card dollar piggyBank trendingUp '
      r'trendingDown barChart pieChart activity target trophy gift book '
      r'bookOpen file folder inbox archive flag dumbbell timer sparkles smile '
      r'car plane checkSquare circle compass repeat history logOut.',
};

/// The kit's whole API as a section of the system prompt.
String kitApiSection() => [
  '## Kit API',
  '',
  'What each file of the kit offers (add_kit copies them; import them by '
      'relative path):',
  '',
  for (final t in kitApi.values) '- $t',
].join('\n');
