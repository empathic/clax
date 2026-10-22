import 'dart:async';
import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:clax_app/platform/bundle_key.dart';
import 'package:cryptography/cryptography.dart';
import 'package:flutter_test/flutter_test.dart';

/// A [BundleSeedStorage] in memory that counts reads and writes.
class MemorySeedStorage implements BundleSeedStorage {
  MemorySeedStorage([this.value]);

  String? value;
  var reads = 0;
  var writes = 0;
  Object? readError;
  Completer<void>? gate;

  @override
  Future<String?> read() async {
    reads++;
    final g = gate;
    if (g != null) await g.future;
    final e = readError;
    if (e != null) throw e;
    return value;
  }

  @override
  Future<void> write(String seed) async {
    writes++;
    value = seed;
  }
}

/// RFC 8032 §7.1 test 1.
final _rfcSeed = Uint8List.fromList(
  _hex('9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60'),
);
final _rfcPublic = _hex(
  'd75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a',
);

List<int> _hex(String s) => [
  for (var i = 0; i < s.length; i += 2)
    int.parse(s.substring(i, i + 2), radix: 16),
];

void main() {
  test('reports no key until one is made, and makes it once', () async {
    final storage = MemorySeedStorage();
    final key = BundleKey(storage, random: Random(1));
    expect(await key.existing(), isNull);

    final made = await key.obtain();
    expect(storage.writes, 1);
    expect(
      base64Url.decode(base64Url.normalize(storage.value!)),
      hasLength(32),
    );
    expect(storage.value, isNot(contains('=')));
    expect(made.publicKey, hasLength(32));
    expect(made.key, matches(RegExp(r'^[A-Za-z0-9_-]{43}$')));

    final again = await key.obtain();
    expect(again.key, made.key);
    expect((await key.existing())!.key, made.key);
    expect(storage.writes, 1);
    expect(storage.reads, 1);
  });

  test('a later process reads the kept seed back', () async {
    final storage = MemorySeedStorage();
    final first = await BundleKey(storage, random: Random(2)).obtain();
    final second = BundleKey(storage, random: Random(3));
    expect((await second.existing())!.key, first.key);
    expect((await second.obtain()).key, first.key);
    expect(storage.writes, 1);
  });

  test('concurrent first exports share one key', () async {
    final storage = MemorySeedStorage()..gate = Completer<void>();
    final key = BundleKey(storage, random: Random.secure());
    final a = key.obtain();
    final b = key.obtain();
    storage.gate!.complete();
    expect((await a).key, (await b).key);
    expect(storage.writes, 1);
    expect(storage.reads, 1);
  });

  test('a failed read is not kept', () async {
    final storage = MemorySeedStorage()..readError = StateError('locked');
    final key = BundleKey(storage, random: Random(4));
    await expectLater(key.existing(), throwsStateError);
    storage.readError = null;
    expect(await key.existing(), isNull);
    expect(storage.reads, 2);
  });

  test('a stored seed that is not 32 bytes is refused, not replaced', () async {
    final storage = MemorySeedStorage('not-a-seed');
    final key = BundleKey(storage, random: Random(5));
    await expectLater(key.obtain(), throwsA(isA<FormatException>()));
    expect(storage.value, 'not-a-seed');
  });

  test('derives the RFC 8032 public key and signs with the seed', () async {
    final storage = MemorySeedStorage(
      base64Url.encode(_rfcSeed).replaceAll('=', ''),
    );
    final id = (await BundleKey(storage).existing())!;
    expect(id.publicKey, _rfcPublic);
    expect(id.key, base64Url.encode(_rfcPublic).replaceAll('=', ''));
    expect(id.seed, _rfcSeed);
    // The empty message's signature, RFC 8032 §7.1 test 1.
    final sig = await Ed25519().sign(
      const [],
      keyPair: await Ed25519().newKeyPairFromSeed(id.seed),
    );
    expect(
      sig.bytes,
      _hex(
        'e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155'
        '5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b',
      ),
    );
    expect(id.toString(), isNot(contains(storage.value!)));
  });

  test('fingerprint: SHA-256 of the key, first 10 bytes, base32 in fours', () {
    final key = base64Url.encode(_rfcPublic).replaceAll('=', '');
    final fp = keyFingerprint(key);
    expect(
      fp,
      matches(RegExp(r'^[A-Z2-7]{4}-[A-Z2-7]{4}-[A-Z2-7]{4}-[A-Z2-7]{4}$')),
    );
    final digest = Sha256().toSync().hashSync(_rfcPublic).bytes.sublist(0, 10);
    expect(fp!.replaceAll('-', ''), base32Upper(digest));
    // RFC 4648 §10 vectors.
    expect(base32Upper(utf8.encode('fooba')), 'MZXW6YTB');
    expect(base32Upper(utf8.encode('foobar')), 'MZXW6YTBOI');
    expect(keyFingerprint('short'), isNull);
    // Non-zero trailing bits are not a canonical 32-byte key.
    expect(keyFingerprint('${'A' * 42}B'), isNull);
  });
}
