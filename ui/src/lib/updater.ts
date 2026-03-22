import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type { Update };

export async function checkForUpdate(): Promise<Update | null> {
  try {
    const update = await check();
    return update ?? null;
  } catch (e) {
    console.error("Update check failed:", e);
    return null;
  }
}

export async function downloadAndInstall(update: Update): Promise<void> {
  await update.downloadAndInstall();
  await relaunch();
}
