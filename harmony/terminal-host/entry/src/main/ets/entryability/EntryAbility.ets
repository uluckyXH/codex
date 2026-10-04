import { UIAbility } from '@kit.AbilityKit';
import { window } from '@kit.ArkUI';
import { fileIo as fs } from '@kit.CoreFileKit';
import hostProbe from 'libhostprobe.so';
export default class EntryAbility extends UIAbility {
  onWindowStageCreate(windowStage: window.WindowStage): void {
    const directory = this.context.getApplicationContext().filesDir;
    windowStage.loadContent('pages/Index');
    try {
      hostProbe.runRequestedCodex(directory).then((result: string) => {
        const done = fs.openSync(directory + '/launch-completion.txt', fs.OpenMode.CREATE | fs.OpenMode.WRITE_ONLY | fs.OpenMode.TRUNC);
        fs.writeSync(done.fd, result); fs.closeSync(done);
      }).catch(() => {});
    } catch (_) {
      // Invalid private action files are rejected; the app remains available for a retry.
    }
  }
}
