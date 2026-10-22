/// Work on bytes that runs on a background isolate once they are large
/// enough to hold the caller's (the UI isolate's) frames.
library;

import 'dart:convert';
import 'dart:isolate';
import 'dart:typed_data';

/// Bytes (or characters) past which the work here runs on a background
/// isolate (`Isolate.run`); smaller ones are done in place, their work
/// bounded by their size.
const offloadBytes = 64 << 10;

/// [base64Encode] of [bytes].
Future<String> base64Off(Uint8List bytes) => bytes.length > offloadBytes
    ? Isolate.run(() => base64Encode(bytes))
    : Future.value(base64Encode(bytes));

/// [base64Decode] of [text]; throws its [FormatException].
Future<Uint8List> base64DecodeOff(String text) => text.length > offloadBytes
    ? Isolate.run(() => base64Decode(text))
    : Future.sync(() => base64Decode(text));
