import 'dart:async';

import 'package:flutter/material.dart';

import '../platform/voice.dart';
import '../platform/voice_cues.dart';

/// Opens the New sheet and returns the request the person submitted, or
/// null when they closed it. [initial] fills the field. An empty sheet
/// starts listening at once when dictation is available and [autoStart]
/// is on (when null, [autoStartOf] the voice: the person's "Start
/// dictation automatically"), so saying what you want is one tap from the
/// launcher. [onChanged] sees every change to the text, typed or heard, so
/// a closed sheet's draft can be kept.
Future<String?> showNewSheet(
  BuildContext context,
  VoiceInput voice,
  String initial, {
  ValueChanged<String>? onChanged,
  bool? autoStart,
}) => showModalBottomSheet<String>(
  context: context,
  isScrollControlled: true,
  useSafeArea: true,
  showDragHandle: true,
  builder: (_) => NewSheet(
    voice: voice,
    initial: initial,
    onChanged: onChanged,
    autoStart: autoStart,
  ),
);

/// The New sheet's body: a text field, a mic button and "Make it".
class NewSheet extends StatefulWidget {
  const NewSheet({
    super.key,
    required this.voice,
    this.initial = '',
    this.onChanged,
    this.autoStart,
  });

  final VoiceInput voice;
  final String initial;
  final ValueChanged<String>? onChanged;

  /// Whether an empty sheet starts listening on its own; when null,
  /// [autoStartOf] [voice].
  final bool? autoStart;

  @override
  State<NewSheet> createState() => _NewSheetState();
}

class _NewSheetState extends State<NewSheet> {
  late final TextEditingController _text = TextEditingController(
    text: widget.initial,
  );
  final _focus = FocusNode();
  VoiceAvailability? _availability;
  StreamSubscription<VoiceResult>? _listen;
  String? _voiceError;

  /// Asks for the voice's availability again when the app comes back to
  /// the front, as after the person changed its access in system settings.
  late final AppLifecycleListener _lifecycle;

  bool get _listening => _listen != null;

  @override
  void initState() {
    super.initState();
    _text.addListener(() {
      widget.onChanged?.call(_text.text);
      if (mounted) setState(() {});
    });
    _lifecycle = AppLifecycleListener(onResume: _recheck);
    widget.voice.availability().then((a) {
      if (!mounted) return;
      setState(() => _availability = a);
      final auto = widget.autoStart ?? autoStartOf(widget.voice);
      if (a is VoiceAvailable && auto && _text.text.trim().isEmpty) {
        _startListening();
      } else if (a is! VoiceAvailable || !auto) {
        _focus.requestFocus();
      }
    });
  }

  Future<void> _recheck() async {
    final a = await widget.voice.availability();
    if (mounted) setState(() => _availability = a);
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    if (_listening) {
      _listen!.cancel();
      widget.voice.stop();
    }
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _startListening() {
    final before = _text.text.trimRight();
    setState(() => _voiceError = null);
    _listen = widget.voice.listen().listen(
      (r) {
        if (!mounted) return;
        final words = r.words.trim();
        final text = before.isEmpty
            ? words
            : (words.isEmpty ? before : '$before $words');
        _text.value = TextEditingValue(
          text: text,
          selection: TextSelection.collapsed(offset: text.length),
        );
      },
      onError: (Object e) {
        if (!mounted) return;
        setState(
          () => _voiceError = e is VoiceError ? e.message : 'Dictation stopped',
        );
      },
      onDone: () {
        if (mounted) setState(() => _listen = null);
      },
    );
  }

  Future<void> _stopListening() async {
    final l = _listen;
    if (l == null) return;
    await widget.voice.stop();
    // The stream closes after the recognizer's last words; drop it now so
    // the button answers at once.
    await l.cancel();
    if (mounted) setState(() => _listen = null);
  }

  void _toggleMic() {
    if (_listening) {
      _stopListening();
    } else {
      _focus.unfocus();
      _startListening();
    }
  }

  void _submit() {
    final request = _text.text.trim();
    if (request.isEmpty) return;
    _stopListening();
    Navigator.of(context).pop(request);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final a = _availability;
    final micReason = switch (a) {
      VoiceDenied(:final reason) || VoiceUnsupported(:final reason) => reason,
      _ => null,
    };
    final micEnabled = a is VoiceAvailable;
    final micTip = micReason ?? (_listening ? 'Stop listening' : 'Speak');
    final status = _voiceError ?? (_listening ? 'Listening…' : null);

    return Padding(
      padding: EdgeInsets.only(
        left: 20,
        right: 20,
        bottom: MediaQuery.viewInsetsOf(context).bottom + 20,
      ),
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text('New app', style: theme.textTheme.titleLarge),
            const SizedBox(height: 16),
            TextField(
              controller: _text,
              focusNode: _focus,
              minLines: 2,
              maxLines: 6,
              textCapitalization: TextCapitalization.sentences,
              textInputAction: TextInputAction.go,
              style: theme.textTheme.bodyLarge,
              decoration: const InputDecoration(
                hintText: 'Say or type what you want',
              ),
              onTap: () {
                if (_listening) _stopListening();
              },
              onChanged: (_) {
                if (_listening) _stopListening();
              },
              onSubmitted: (_) => _submit(),
            ),
            if (micReason != null)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 8),
                child: Row(
                  children: [
                    Icon(
                      Icons.mic_off_rounded,
                      size: 18,
                      color: scheme.onSurfaceVariant,
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        micReason,
                        style: theme.textTheme.bodyMedium?.copyWith(
                          color: scheme.onSurfaceVariant,
                        ),
                      ),
                    ),
                    if (a is VoiceDenied)
                      TextButton(
                        onPressed: widget.voice.openPermissionSettings,
                        child: const Text('Open settings'),
                      ),
                  ],
                ),
              )
            else
              SizedBox(
                height: 32,
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: AnimatedSwitcher(
                    duration: const Duration(milliseconds: 150),
                    child: status == null
                        ? const SizedBox.shrink()
                        : Text(
                            status,
                            key: ValueKey(status),
                            style: theme.textTheme.bodyMedium?.copyWith(
                              color: _voiceError != null
                                  ? scheme.error
                                  : scheme.primary,
                            ),
                          ),
                  ),
                ),
              ),
            Row(
              children: [
                Tooltip(
                  message: micTip,
                  child: IconButton.filled(
                    key: const Key('new-sheet-mic'),
                    iconSize: 28,
                    style: IconButton.styleFrom(
                      fixedSize: const Size(64, 64),
                      backgroundColor: _listening
                          ? scheme.primary
                          : scheme.primaryContainer,
                      foregroundColor: _listening
                          ? scheme.onPrimary
                          : scheme.onPrimaryContainer,
                    ),
                    onPressed: micEnabled ? _toggleMic : null,
                    icon: Icon(
                      _listening
                          ? Icons.stop_rounded
                          : (micEnabled
                                ? Icons.mic_rounded
                                : Icons.mic_off_rounded),
                    ),
                  ),
                ),
                const Spacer(),
                FilledButton.icon(
                  style: FilledButton.styleFrom(
                    minimumSize: const Size(0, 52),
                    padding: const EdgeInsets.symmetric(horizontal: 24),
                  ),
                  onPressed: _text.text.trim().isEmpty ? null : _submit,
                  icon: const Icon(Icons.auto_awesome_rounded),
                  label: const Text('Make it'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}
