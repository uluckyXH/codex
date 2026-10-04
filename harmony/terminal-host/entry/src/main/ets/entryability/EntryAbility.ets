import { UIAbility } from '@kit.AbilityKit';
import { window } from '@kit.ArkUI';
import hostProbe from 'libhostprobe.so';

export default class EntryAbility extends UIAbility {
  onWindowStageCreate(windowStage: window.WindowStage): void {
    windowStage.loadContent('pages/Index');
    try {
      // Native code writes the completion atomically beneath pinned host/.
      // With no action file the only action is offline directory preparation.
      hostProbe.runRequestedCodex(this.context.getApplicationContext().filesDir).then((result: string) => {
        AppStorage.setOrCreate('hostLastAction', result);
      }).catch(() => { AppStorage.setOrCreate('hostLastAction', 'error: native action rejected'); });
    } catch (_) {
      // The UI remains available for explicit protected-directory repair/retry.
    }
  }
}
