import { describe, it, expect, vi, beforeEach } from "vitest";

let mockIsPermissionGranted: ReturnType<typeof vi.fn>;
let mockRequestPermission: ReturnType<typeof vi.fn>;
let mockSendNotification: ReturnType<typeof vi.fn>;
let notifyDesktop: (title: string, body: string) => Promise<void>;

beforeEach(async () => {
  vi.resetModules();
  mockIsPermissionGranted = vi.fn();
  mockRequestPermission = vi.fn();
  mockSendNotification = vi.fn();
  vi.doMock("@tauri-apps/plugin-notification", () => ({
    isPermissionGranted: mockIsPermissionGranted,
    requestPermission: mockRequestPermission,
    sendNotification: mockSendNotification,
  }));
  const mod = await import("../lib/desktopNotifications");
  notifyDesktop = mod.notifyDesktop;
});

describe("notifyDesktop", () => {
  describe("when permission is already granted", () => {
    it("calls sendNotification", async () => {
      // Given: permission is granted
      mockIsPermissionGranted.mockResolvedValue(true);

      // When: call notifyDesktop
      await notifyDesktop("Cruise", "Completed");

      // Then: sendNotification is called with correct arguments
      expect(mockSendNotification).toHaveBeenCalledOnce();
      expect(mockSendNotification).toHaveBeenCalledWith({
        title: "Cruise",
        body: "Completed",
      });
    });

    it("does not call requestPermission", async () => {
      // Given: permission is granted
      mockIsPermissionGranted.mockResolvedValue(true);

      // When
      await notifyDesktop("Cruise", "test");

      // Then: does not request additional permission
      expect(mockRequestPermission).not.toHaveBeenCalled();
    });
  });

  describe("when permission is denied", () => {
    it("calls requestPermission and does not call sendNotification if still denied", async () => {
      // Given: permission is denied, and request is also denied
      mockIsPermissionGranted.mockResolvedValue(false);
      mockRequestPermission.mockResolvedValue("denied");

      // When
      await notifyDesktop("Cruise", "Completed");

      // Then: not sent
      expect(mockSendNotification).not.toHaveBeenCalled();
    });

    it("calls sendNotification if requestPermission returns granted", async () => {
      // Given: initially denied but user grants permission
      mockIsPermissionGranted.mockResolvedValue(false);
      mockRequestPermission.mockResolvedValue("granted");

      // When
      await notifyDesktop("Cruise", "Completed");

      // Then: sent
      expect(mockSendNotification).toHaveBeenCalledOnce();
    });
  });

  describe("permission result caching", () => {
    it("does not call isPermissionGranted again on second call", async () => {
      // Given: permission is granted
      mockIsPermissionGranted.mockResolvedValue(true);

      // When: called twice
      await notifyDesktop("Cruise", "first");
      await notifyDesktop("Cruise", "second");

      // Then: isPermissionGranted is called only once
      expect(mockIsPermissionGranted).toHaveBeenCalledOnce();
      expect(mockSendNotification).toHaveBeenCalledTimes(2);
    });
  });

  describe("error handling: silent fail", () => {
    it("does not propagate error when isPermissionGranted throws", async () => {
      // Given: permission check fails
      mockIsPermissionGranted.mockRejectedValue(new Error("Notification API unavailable"));

      // When / Then: no error is thrown
      await expect(notifyDesktop("Cruise", "test")).resolves.toBeUndefined();
    });

    it("does not propagate error when sendNotification throws", async () => {
      // Given: permission is granted but sendNotification fails
      mockIsPermissionGranted.mockResolvedValue(true);
      mockSendNotification.mockImplementation(() => {
        throw new Error("Failed to send notification");
      });

      // When / Then: no error is thrown
      await expect(notifyDesktop("Cruise", "test")).resolves.toBeUndefined();
    });

    it("does not propagate error when requestPermission throws", async () => {
      // Given: permission is denied and requestPermission throws
      mockIsPermissionGranted.mockResolvedValue(false);
      mockRequestPermission.mockRejectedValue(new Error("Permission dialog failed"));

      // When / Then: no error is thrown
      await expect(notifyDesktop("Cruise", "test")).resolves.toBeUndefined();
    });
  });
});
