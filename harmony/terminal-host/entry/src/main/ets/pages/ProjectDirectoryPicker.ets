import { common } from '@kit.AbilityKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { Environment, fileIo, fileUri, picker } from '@kit.CoreFileKit';

export interface ProjectDirectorySelection {
  cancelled: boolean;
  uri: string;
  path: string;
  readWriteVerified: boolean;
}

export class ProjectDirectoryError extends Error {
  stage: string;
  code: number;

  constructor(stage: string, message: string, code: number = 0) {
    super(message);
    this.stage = stage;
    this.code = code;
  }
}

function directoryPath(path: string): void {
  if (typeof path !== 'string' || !path.startsWith('/') || path === '/' || /[\u0000-\u001f\u007f]/.test(path)) {
    throw new ProjectDirectoryError('uri_path', '系统返回的目录不是可用的绝对路径。');
  }
  const components: string[] = path.split('/');
  for (let index: number = 1; index < components.length; index++) {
    if (components[index] === '' || components[index] === '.' || components[index] === '..') {
      throw new ProjectDirectoryError('uri_path', '系统返回的目录包含空段或相对路径段。');
    }
  }
}

function desktopHint(): string {
  // This query is only an initial Picker location, never a permission grant.
  // Do not manufacture a /storage path from a UID or use the HDC view.
  try {
    if (!canIUse('SystemCapability.FileManagement.File.Environment.FolderObtain')) return '';
    const desktop: string = Environment.getUserDesktopDir();
    directoryPath(desktop);
    return fileUri.getUriFromPath(desktop);
  } catch (_) {
    return '';
  }
}

function platformError(stage: string, error: BusinessError): ProjectDirectoryError {
  return new ProjectDirectoryError(stage, error.message,
    Number.isFinite(error.code) ? error.code : 0);
}

function requireFolderSelection(): void {
  if (!canIUse('SystemCapability.FileManagement.UserFileService.FolderSelection')) {
    throw new ProjectDirectoryError('picker_capability', '当前系统不支持文件夹选择。', 801);
  }
}

function sandboxDirectoryPath(selectedUri: string): string {
  let path: string;
  try {
    const selected = new fileUri.FileUri(selectedUri);
    // URI parts may be null at runtime even though SDK26 declares string.
    if (selected.scheme !== 'file' || selected.isRemoteUri() ||
      (selected.query ?? '').length > 0 || (selected.fragment ?? '').length > 0) {
      throw new ProjectDirectoryError('uri_kind', '所选目录不是受支持的本地文件夹。');
    }
    // FileUri.path applies the platform conversion and decodes exactly once.
    // It does not verify access; never slice the URI or decode this path again.
    path = selected.path;
    directoryPath(path);
  } catch (error) {
    if (error instanceof ProjectDirectoryError) throw error;
    throw platformError('uri_path', error as BusinessError);
  }
  return path;
}

async function checkDirectory(path: string): Promise<void> {
  try {
    const directory = await fileIo.open(path,
      fileIo.OpenMode.READ_ONLY | fileIo.OpenMode.DIR | fileIo.OpenMode.NOFOLLOW);
    try {
      if (!fileIo.statSync(directory.fd).isDirectory()) {
        throw new ProjectDirectoryError('open_path', '所选路径不是目录。');
      }
    } finally {
      fileIo.closeSync(directory);
    }
  } catch (error) {
    if (error instanceof ProjectDirectoryError) throw error;
    throw platformError('open_path', error as BusinessError);
  }
}

export async function selectProjectDirectory(context: common.UIAbilityContext): Promise<ProjectDirectorySelection> {
  requireFolderSelection();
  const options = new picker.DocumentSelectOptions();
  options.selectMode = picker.DocumentSelectMode.FOLDER;
  options.maxSelectNumber = 1;
  options.allowsMulFolderSelection = false;
  options.authMode = false;
  const hint: string = desktopHint();
  if (hint.length > 0) options.defaultFilePathUri = hint;
  const viewPicker = new picker.DocumentViewPicker(context);
  let uris: string[];
  try {
    uris = await viewPicker.select(options);
  } catch (error) {
    throw platformError('picker_select', error as BusinessError);
  }
  if (uris.length === 0) return { cancelled: true, uri: '', path: '', readWriteVerified: false };
  if (uris.length !== 1) {
    throw new ProjectDirectoryError('picker_result', '请选择一个项目文件夹。');
  }
  const selectedUri: string = uris[0];
  const path: string = sandboxDirectoryPath(selectedUri);
  await checkDirectory(path);
  // This validates access in the application only. Native cwd/read permissions
  // remain enforced by terminalStart and the actual child process.
  return { cancelled: false, uri: selectedUri, path: path, readWriteVerified: false };
}

export async function authorizeProjectDirectory(context: common.UIAbilityContext,
  selectedUri: string, expectedPath: string): Promise<ProjectDirectorySelection> {
  requireFolderSelection();
  // Authorization must use the exact URI already chosen by the user. It must
  // not use desktopHint, reconstruct a URI from cwd, or authorize a parent.
  if (selectedUri.length === 0 || sandboxDirectoryPath(selectedUri) !== expectedPath) {
    throw new ProjectDirectoryError('authorization_scope', '请先重新选择要授权的项目文件夹。');
  }
  const options = new picker.DocumentSelectOptions();
  options.selectMode = picker.DocumentSelectMode.FOLDER;
  options.maxSelectNumber = 1;
  options.allowsMulFolderSelection = false;
  options.authMode = true;
  options.defaultFilePathUri = selectedUri;
  const viewPicker = new picker.DocumentViewPicker(context);
  let uris: string[];
  try {
    uris = await viewPicker.select(options);
  } catch (error) {
    throw platformError('picker_authorization', error as BusinessError);
  }
  if (uris.length === 0) return { cancelled: true, uri: '', path: '', readWriteVerified: false };
  if (uris.length !== 1 || sandboxDirectoryPath(uris[0]) !== expectedPath) {
    throw new ProjectDirectoryError('authorization_scope', '授权结果与所选项目文件夹不一致。');
  }
  await checkDirectory(expectedPath);
  try {
    // Picker success alone is not evidence of write permission. This query
    // performs no create/write/delete operation and does not grant permission.
    const accessible: boolean = await fileIo.access(expectedPath,
      fileIo.AccessModeType.READ_WRITE, fileIo.AccessFlagType.LOCAL);
    if (!accessible) {
      throw new ProjectDirectoryError('read_write_access', '所选项目未通过本地读写权限检查。', 13900012);
    }
  } catch (error) {
    if (error instanceof ProjectDirectoryError) throw error;
    throw platformError('read_write_access', error as BusinessError);
  }
  // Native child CRUD remains a device acceptance check, even after access().
  return { cancelled: false, uri: uris[0], path: expectedPath, readWriteVerified: true };
}
