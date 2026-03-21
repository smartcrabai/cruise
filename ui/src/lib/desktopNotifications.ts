import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

let _permissionPromise: Promise<boolean> | null = null;

function ensurePermission(): Promise<boolean> {
  if (_permissionPromise === null) {
    _permissionPromise = isPermissionGranted().then(async (granted) => {
      if (!granted) {
        const result = await requestPermission();
        return result === "granted";
      }
      return granted;
    }).catch((err: unknown) => {
      _permissionPromise = null;
      throw err;
    });
  }
  return _permissionPromise;
}

export async function notifyDesktop(title: string, body: string): Promise<void> {
  try {
    const granted = await ensurePermission();
    if (granted) await sendNotification({ title, body });
  } catch {
    // Silently fail in environments where notifications are not available
  }
}
