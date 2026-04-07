import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type { Update };

export async function checkForUpdate(): Promise<Update | null> {
  try {
    return await check();
  } catch (e) {
    console.error("Update check failed:", e);
    return null;
  }
}

/** Like checkForUpdate but propagates errors -- use for manual user-initiated checks. */
export async function checkForUpdateManual(): Promise<Update | null> {
  return check();
}

export async function downloadAndInstall(update: Update): Promise<void> {
  await update.downloadAndInstall();
  await relaunch();
}
