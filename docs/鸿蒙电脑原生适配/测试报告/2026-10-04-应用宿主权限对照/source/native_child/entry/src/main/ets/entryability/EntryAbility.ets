import { UIAbility } from '@kit.AbilityKit';
import { window } from '@kit.ArkUI';
import { fileIo as fs } from '@kit.CoreFileKit';
import hostProbe from 'libhostprobe.so';
export default class EntryAbility extends UIAbility {
  onWindowStageCreate(windowStage: window.WindowStage): void {
    const result = JSON.stringify({filesDir:this.context.filesDir,cacheDir:this.context.cacheDir,tempDir:this.context.tempDir});
    const file = fs.openSync(this.context.filesDir + '/context.json', fs.OpenMode.CREATE | fs.OpenMode.WRITE_ONLY | fs.OpenMode.TRUNC);
    fs.writeSync(file.fd, result);
    fs.closeSync(file);
    windowStage.loadContent('pages/Index');
    hostProbe.runProbe(this.context.filesDir).then((result: string) => {
      const done = fs.openSync(this.context.filesDir + '/probe-completion.txt', fs.OpenMode.CREATE | fs.OpenMode.WRITE_ONLY | fs.OpenMode.TRUNC);
      fs.writeSync(done.fd, result);
      fs.closeSync(done);
    });
  }
}
