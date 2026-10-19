import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

/// The bundled asset with Clax's own licence: `apps/clax_app/LICENSE`, the
/// app's GPL-2.0-only notice and the licence's full text.
const claxLicenceAsset = 'LICENSE';

/// The copyright and licence the licences page shows under the app's name.
const claxLegalese =
    'Copyright (C) 2026 Empathic, Inc.\n'
    'Licensed under the GNU General Public License, version 2 only '
    '(GPL-2.0-only).';

bool _registered = false;

/// Adds Clax's own licence ([claxLicenceAsset]) to [LicenseRegistry], under
/// the package name "Clax", beside the licences of the packages the app
/// is built from. Once per process; later calls do nothing.
void registerClaxLicence() {
  if (_registered) return;
  _registered = true;
  LicenseRegistry.addLicense(() async* {
    final text = await rootBundle.loadString(claxLicenceAsset);
    yield LicenseEntryWithLineBreaks(const ['Clax'], text);
  });
}

/// Shows the licences page: Clax with [claxLegalese], then every licence
/// in [LicenseRegistry] ([registerClaxLicence] adds Clax's own).
void showClaxLicences(BuildContext context) {
  registerClaxLicence();
  showLicensePage(
    context: context,
    applicationName: 'Clax',
    applicationLegalese: claxLegalese,
  );
}
