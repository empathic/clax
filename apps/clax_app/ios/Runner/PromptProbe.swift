#if DEBUG
import AVFAudio
import CoreLocation
import Speech

/// Debug builds only: raises one of iOS's permission prompts at launch, so
/// a simulator run can show the words Info.plist gives it without tapping
/// through Clax. `xcrun simctl launch <udid> dev.empathic.clax
/// -ClaxAskPermission <mic|speech|location>`; without the argument it does
/// nothing.
enum PromptProbe {
  private static var location: CLLocationManager?

  static func askIfLaunchedTo() {
    switch UserDefaults.standard.string(forKey: "ClaxAskPermission") {
    case "mic":
      AVAudioApplication.requestRecordPermission { _ in }
    case "speech":
      SFSpeechRecognizer.requestAuthorization { _ in }
    case "location":
      let manager = CLLocationManager()
      location = manager
      manager.requestWhenInUseAuthorization()
    default:
      break
    }
  }
}
#endif
