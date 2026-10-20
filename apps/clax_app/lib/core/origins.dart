import 'dart:io';

/// The origins the app's WebViews load from, which the app answers through
/// the core: the shell's, and each artifact's content origin. The app
/// writes them to `<home>/embed.json` ([toJson]) before it opens the core.
class AppOrigins {
  const AppOrigins(this.shell, this.contentTemplate);

  /// This platform's: `https://….clax.invalid` hosts on Android (where
  /// WebView intercepts only http and https), a `clax` scheme on Apple.
  /// [android] overrides the platform.
  static AppOrigins forPlatform({bool? android}) =>
      (android ?? Platform.isAndroid)
      ? const AppOrigins(
          'https://app.clax.invalid',
          'https://{id}.c.clax.invalid',
        )
      : const AppOrigins('clax://app', 'clax://{id}.c');

  /// The shell's origin.
  final String shell;

  /// An artifact's content origin, with `{id}` for its ID.
  final String contentTemplate;

  /// The content origin of artifact [id].
  String content(String id) => contentTemplate.replaceAll('{id}', id);

  /// The artifact ID a content origin's [host] names, or null for any
  /// other host.
  String? idOfHost(String host) {
    // The template's host is `{id}` then a fixed suffix.
    final suffix = contentTemplate.substring(
      contentTemplate.indexOf('{id}') + '{id}'.length,
    );
    if (!host.endsWith(suffix)) return null;
    final id = host.substring(0, host.length - suffix.length);
    return _isArtifactId(id) ? id : null;
  }

  /// Whether [host] is the shell's.
  bool isShellHost(String host) => host == Uri.parse(shell).host;

  Map<String, String> toJson() => {'shell': shell, 'content': contentTemplate};

  /// 12 characters of lowercase Crockford base32
  /// (crates/clax-core/src/ids.rs).
  static bool _isArtifactId(String s) =>
      s.length == 12 && s.codeUnits.every(_alphabet.codeUnits.contains);

  static const _alphabet = '0123456789abcdefghjkmnpqrstvwxyz';
}
