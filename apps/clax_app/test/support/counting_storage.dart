import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// A [FlutterSecureStorage] that keeps every call made on it in [calls]
/// and answers each with the saved key `sk-saved`: a read or write that
/// reached the storage shows there.
class CountingStorage implements FlutterSecureStorage {
  final calls = <Symbol>[];

  @override
  dynamic noSuchMethod(Invocation invocation) {
    calls.add(invocation.memberName);
    return Future<String?>.value('sk-saved');
  }
}
