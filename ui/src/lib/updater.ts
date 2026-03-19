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

export async function downloadAndInstall(
  update: Update,
  onProgress?: (chunkLength: number, contentLength: number | undefined) => void,
): Promise<void> {
  await update.downloadAndInstall((event) => {
    if (event.event === "Started") {
      onProgress?.(0, event.data.contentLength);
    } else if (event.event === "Progress") {
      onProgress?.(event.data.chunkLength, undefined);
    }
  });
  await relaunch();
}
