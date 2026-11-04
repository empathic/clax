// The timing rules for the Clax app's Dart tests (test/ and
// integration_test/): a test waits on a condition it names, never on a
// stretch of real time or a count of event-loop turns, which run out first
// on a loaded machine. Fake time (`tester.pump(d)` in a `testWidgets`, an
// injected `Clock`) is no wall clock and is allowed.
//
// Each rule reads one file's syntax tree (package:analyzer's parser, no type
// resolution), so a rule names calls by how they are written: `Future.delayed`
// is any `Future.delayed(...)`, `pump` any `x.pump(...)`.
//
// A finding is exempted by a comment on its line, or alone on the line
// before it, naming the rule and saying why:
//
//   // ignore: timing_wall_clock -- the hang guard's own deadline
//
// The Dart analyzer reads the same comment and ignores the unknown name. An
// exemption without a reason, or one whose line has no such finding, is a
// finding of its own (`timing_ignore_reason`, `timing_stale_ignore`).
import 'package:analyzer/dart/analysis/utilities.dart';
import 'package:analyzer/dart/ast/ast.dart';
import 'package:analyzer/dart/ast/token.dart';
import 'package:analyzer/dart/ast/visitor.dart';
import 'package:analyzer/source/line_info.dart';

/// The rules, with what to do instead.
const rules = <String, String>{
  'timing_sleep':
      'sleep() blocks for a fixed time: wait on the condition '
      '(waitUntil, settleUntil, rig.until)',
  'timing_delay':
      'a real fixed delay: wait on the condition (settleUntil, waitUntil, '
      'rig.until), or drive the time with a FakeClock',
  'timing_pump':
      'pump(d) waits d of real time here: wait until the state holds or the '
      'widget is hit-testable (rig.until, settleUntil)',
  'timing_wall_clock':
      'reads the wall clock: use an injected Clock (FakeClock, HeldClock), '
      'or the stuckAfter-guarded waits of test/support/settle.dart',
  'timing_short_timeout':
      'a timeout below stuckAfter decides a result under load: use '
      'stuckAfter, or no timeout',
  'timing_turn_count':
      'a counted number of event-loop turns runs out first under load: wait '
      'on the condition (waitUntil, settleUntil, FakeClock.advance(idle:))',
  'timing_ignore_reason':
      'an exemption says why: // ignore: <rule> -- <reason>',
  'timing_stale_ignore':
      'this exemption exempts nothing: remove it (or name the right rule)',
};

/// One rule broken at one line (1-based).
class Finding {
  Finding(this.path, this.line, this.rule);

  final String path;
  final int line;
  final String rule;

  String get hint => rules[rule]!;

  @override
  String toString() => '$path:$line: $rule: $hint';
}

/// The findings in [source], the file at [path] (a path whose components
/// say whether it is an integration test: any under `integration_test/`).
/// [stuckAfter] is the hang guard below which a literal timeout is short.
List<Finding> lint(String path, String source, {required Duration stuckAfter}) {
  final parsed = parseString(
    content: source,
    path: path,
    throwIfDiagnostics: false,
  );
  final visitor = _Rules(
    path: path,
    lines: parsed.lineInfo,
    integration: path.split('/').contains('integration_test'),
    stuckAfter: stuckAfter,
  );
  parsed.unit.accept(visitor);
  return _applyIgnores(path, source, visitor.found);
}

/// The value of `const stuckAfter = Duration(...)` in [source]
/// (test/support/settle.dart), or null.
Duration? stuckAfterIn(String source) {
  final unit = parseString(content: source, throwIfDiagnostics: false).unit;
  for (final d in unit.declarations.whereType<TopLevelVariableDeclaration>()) {
    for (final v in d.variables.variables) {
      if (v.name.lexeme == 'stuckAfter' && v.initializer != null) {
        return _duration(v.initializer!, const {});
      }
    }
  }
  return null;
}

class _Rules extends RecursiveAstVisitor<void> {
  _Rules({
    required this.path,
    required this.lines,
    required this.integration,
    required this.stuckAfter,
  });

  final String path;
  final LineInfo lines;
  final bool integration;
  final Duration stuckAfter;
  final found = <Finding>[];

  /// The durations this file's variables are initialized to, by name
  /// (whatever their scope), so `.timeout(_cap)` reads `_cap`'s value.
  final _named = <String, Duration>{};

  /// A finding at [at]'s line; a method call's is its name's (`.timeout`
  /// on the line it is written, not where its receiver starts).
  void _report(AstNode at, String rule) {
    final where = at is MethodInvocation ? at.methodName : at;
    found.add(Finding(path, lines.getLocation(where.offset).lineNumber, rule));
  }

  @override
  void visitCompilationUnit(CompilationUnit node) {
    for (final v in _all<VariableDeclaration>(node)) {
      final init = v.initializer;
      if (init == null) continue;
      final d = _duration(init, const {});
      if (d != null) _named[v.name.lexeme] = d;
    }
    super.visitCompilationUnit(node);
  }

  @override
  void visitMethodInvocation(MethodInvocation node) {
    final target = node.target;
    final type = target == null
        ? null
        : target is SimpleIdentifier
        ? target.name
        : '';
    _call(node, type, node.methodName.name, node.argumentList);
    super.visitMethodInvocation(node);
  }

  @override
  void visitInstanceCreationExpression(InstanceCreationExpression node) {
    final c = node.constructorName;
    _call(node, c.type.name.lexeme, c.name?.name ?? '', node.argumentList);
    super.visitInstanceCreationExpression(node);
  }

  /// A call written `type.name(args)`: [type] is null for a bare
  /// `name(args)`, the identifier before the dot (`Future`, `tester`), or ''
  /// for a call on another expression; an unnamed constructor written with
  /// `new` or `const` has the class as [type] and '' as [name].
  void _call(AstNode node, String? type, String name, ArgumentList args) {
    final first = args.arguments.isEmpty
        ? null
        : args.arguments.first.argumentExpression;
    final realTime = integration || _inRunAsync(node);
    if (type == null && name == 'sleep' && args.arguments.length == 1) {
      _report(node, 'timing_sleep');
    } else if (type == 'Future' && name == 'delayed') {
      final d = first == null ? Duration.zero : _duration(first, _named);
      if (realTime && d != Duration.zero && !_inClock(node)) {
        _report(node, 'timing_delay');
      }
    } else if (type != null &&
        name == 'pump' &&
        first != null &&
        args.arguments.first is! NamedArgument) {
      final d = _duration(first, _named);
      if (realTime && d != Duration.zero) _report(node, 'timing_pump');
    } else if ((type == 'DateTime' && name == 'now') ||
        (type == null && name == 'Stopwatch') ||
        (type == 'Stopwatch' && name == '')) {
      if (!_inClock(node)) _report(node, 'timing_wall_clock');
    } else if (type != null &&
        name == 'timeout' &&
        first != null &&
        args.arguments.first is! NamedArgument) {
      if (_short(first)) _report(node, 'timing_short_timeout');
    } else if (type == null && name == 'pumpEventQueue') {
      final times = _namedArgument(args, 'times');
      if (times is IntegerLiteral) _report(node, 'timing_turn_count');
    }
  }

  @override
  void visitNamedArgument(NamedArgument node) {
    if (node.name.lexeme == 'timeout' && _short(node.argumentExpression)) {
      _report(node, 'timing_short_timeout');
    }
    super.visitNamedArgument(node);
  }

  @override
  void visitForStatement(ForStatement node) {
    final parts = node.forLoopParts;
    if (parts is ForParts &&
        _literalBound(parts.condition) &&
        _onlyTurns(node.body)) {
      _report(node, 'timing_turn_count');
    }
    super.visitForStatement(node);
  }

  /// Whether [e] is a literal duration (or `Timeout(<duration>)`) shorter
  /// than [stuckAfter].
  bool _short(Expression e) {
    var d = _duration(e, _named);
    if (d == null) {
      final inner = _timeoutArgument(e);
      if (inner != null) d = _duration(inner, _named);
    }
    return d != null && d < stuckAfter;
  }

  /// Whether [body] does nothing but wait: each of its statements awaits
  /// an event-loop turn or a pump, and one at least a turn (so a loop that
  /// presses a key and pumps after each press is no wait).
  bool _onlyTurns(Statement body) {
    final statements = body is Block ? body.statements : [body];
    var turns = 0;
    for (final st in statements) {
      if (st is! ExpressionStatement) return false;
      var e = st.expression;
      if (e is AwaitExpression) e = e.expression;
      if (_isTurn(e)) {
        turns++;
      } else if (!(e is MethodInvocation &&
          e.target != null &&
          e.methodName.name == 'pump')) {
        return false;
      }
    }
    return turns > 0;
  }

  /// Whether [e] is one event-loop turn that no fake clock moves: a zero
  /// delay, a frameless pump, `pumpEventQueue`, `nextTurn`,
  /// `Future.microtask`, or a `runAsync`.
  bool _isTurn(Expression e) {
    bool zero(NodeList<Argument> args) {
      final positional = args.where((a) => a is! NamedArgument);
      return positional.isEmpty ||
          _duration(positional.first.argumentExpression, _named) ==
              Duration.zero;
    }

    if (e is MethodInvocation) {
      final name = e.methodName.name;
      final target = e.target;
      if (target == null && (name == 'pumpEventQueue' || name == 'nextTurn'))
        return true;
      if (target != null && name == 'runAsync') return true;
      if (target != null && name == 'pump')
        return zero(e.argumentList.arguments);
      if (target is SimpleIdentifier && target.name == 'Future') {
        return name == 'microtask' ||
            (name == 'delayed' && zero(e.argumentList.arguments));
      }
    } else if (e is InstanceCreationExpression) {
      final c = e.constructorName;
      return c.type.name.lexeme == 'Future' &&
          c.name?.name == 'delayed' &&
          zero(e.argumentList.arguments);
    }
    return false;
  }
}

/// Every node of type [T] under [root].
List<T> _all<T extends AstNode>(AstNode root) {
  final out = <T>[];
  root.accept(
    _Collect((n) {
      if (n is T) out.add(n);
    }),
  );
  return out;
}

class _Collect extends GeneralizingAstVisitor<void> {
  _Collect(this.each);

  final void Function(AstNode) each;

  @override
  void visitNode(AstNode node) {
    each(node);
    super.visitNode(node);
  }
}

/// `for (...; i < 20; ...)`: a condition comparing with an integer literal.
bool _literalBound(Expression? condition) {
  if (condition is! BinaryExpression) return false;
  final op = condition.operator.type;
  if (op == TokenType.LT || op == TokenType.LT_EQ) {
    return condition.rightOperand is IntegerLiteral;
  }
  if (op == TokenType.GT || op == TokenType.GT_EQ) {
    return condition.leftOperand is IntegerLiteral;
  }
  return false;
}

bool _inRunAsync(AstNode node) {
  for (AstNode? n = node.parent; n != null; n = n.parent) {
    if (n is FunctionExpression && n.parent is ArgumentList) {
      final call = n.parent!.parent;
      if (call is MethodInvocation && call.methodName.name == 'runAsync')
        return true;
    }
  }
  return false;
}

/// Inside a class that is a `Clock` (implements, extends or mixes it in):
/// the clocks are where real time is allowed to be read.
bool _inClock(AstNode node) {
  final c = node.thisOrAncestorOfType<ClassDeclaration>();
  if (c == null) return false;
  final types = [
    ...?c.implementsClause?.interfaces,
    if (c.extendsClause != null) c.extendsClause!.superclass,
    ...?c.withClause?.mixinTypes,
  ];
  return types.any((t) => t.name.lexeme == 'Clock');
}

Expression? _namedArgument(ArgumentList args, String name) {
  for (final a in args.arguments) {
    if (a is NamedArgument && a.name.lexeme == name)
      return a.argumentExpression;
  }
  return null;
}

/// `Timeout(<duration>)`'s argument, or null.
Expression? _timeoutArgument(Expression e) {
  if (e is InstanceCreationExpression &&
      e.constructorName.type.name.lexeme == 'Timeout' &&
      e.constructorName.name == null &&
      e.argumentList.arguments.length == 1) {
    return e.argumentList.arguments.first.argumentExpression;
  }
  if (e is MethodInvocation &&
      e.target == null &&
      e.methodName.name == 'Timeout' &&
      e.argumentList.arguments.length == 1) {
    return e.argumentList.arguments.first.argumentExpression;
  }
  return null;
}

const _units = {
  'days': Duration(days: 1),
  'hours': Duration(hours: 1),
  'minutes': Duration(minutes: 1),
  'seconds': Duration(seconds: 1),
  'milliseconds': Duration(milliseconds: 1),
  'microseconds': Duration(microseconds: 1),
};

/// The value of a duration written with literals (`Duration.zero`,
/// `const Duration(seconds: 5)`, or a name in [named]), or null.
Duration? _duration(Expression e, Map<String, Duration> named) {
  if (e is ParenthesizedExpression) return _duration(e.expression, named);
  if (e is PrefixedIdentifier &&
      e.prefix.name == 'Duration' &&
      e.identifier.name == 'zero') {
    return Duration.zero;
  }
  if (e is SimpleIdentifier) return named[e.name];
  ArgumentList? args;
  if (e is InstanceCreationExpression &&
      e.constructorName.type.name.lexeme == 'Duration' &&
      e.constructorName.name == null) {
    args = e.argumentList;
  } else if (e is MethodInvocation &&
      e.target == null &&
      e.methodName.name == 'Duration') {
    args = e.argumentList;
  }
  if (args == null) return null;
  var total = Duration.zero;
  for (final a in args.arguments) {
    if (a is! NamedArgument) return null;
    final unit = _units[a.name.lexeme];
    final value = a.argumentExpression;
    if (unit == null || value is! IntegerLiteral || value.value == null)
      return null;
    total += unit * value.value!;
  }
  return total;
}

final _ignore = RegExp(
  r'//\s*ignore:\s*([a-z0-9_]+(?:\s*,\s*[a-z0-9_]+)*)(.*)$',
);

/// An `// ignore:` of one timing rule: written at [line], exempting
/// findings at [target].
class _Ignore {
  _Ignore(this.line, this.target, this.rule);

  final int line;
  final int target;
  final String rule;
  var used = false;
}

/// Drops each finding an `// ignore:` comment exempts, and reports the
/// exemptions that say no reason or exempt nothing.
List<Finding> _applyIgnores(String path, String source, List<Finding> found) {
  final lines = source.split('\n');
  final out = <Finding>[];
  final ignores = <_Ignore>[];
  for (var i = 0; i < lines.length; i++) {
    final m = _ignore.firstMatch(lines[i]);
    if (m == null) continue;
    final names = m[1]!
        .split(',')
        .map((s) => s.trim())
        .where((s) => s.startsWith('timing_'));
    if (names.isEmpty) continue;
    final reason = m[2]!
        .replaceFirst(RegExp(r'^[\s\-\u2013\u2014:(]+'), '')
        .trim();
    if (reason.isEmpty) {
      out.add(Finding(path, i + 1, 'timing_ignore_reason'));
      continue;
    }
    // Alone on its line, it exempts the next line; after code, its own.
    final target = lines[i].trimLeft().startsWith('//') ? i + 2 : i + 1;
    ignores.addAll(names.map((n) => _Ignore(i + 1, target, n)));
  }
  for (final f in found) {
    final by = ignores.where((x) => x.target == f.line && x.rule == f.rule);
    if (by.isEmpty) {
      out.add(f);
    } else {
      for (final x in by) {
        x.used = true;
      }
    }
  }
  for (final x in ignores.where((x) => !x.used)) {
    out.add(Finding(path, x.line, 'timing_stale_ignore'));
  }
  out.sort((a, b) => a.line.compareTo(b.line));
  return out;
}
